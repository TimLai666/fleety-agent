//! Speech for voice mode: text-to-speech (`speak`) and speech-to-text (`listen`).
//!
//! Everything here degrades to plain text and never panics. The server only ever
//! exchanges text — STT/TTS happen entirely on this terminal:
//!
//! - **TTS** (`speak`): OS-native — macOS `say`, Windows SAPI (via PowerShell),
//!   Linux `spd-say` (speech-dispatcher).
//! - **STT** (`listen`): cross-platform via a local transcription engine
//!   (default whisper.cpp) — record the microphone with `cpal`, write a 16 kHz
//!   mono WAV, and run a configurable transcribe command. If that is
//!   unavailable it falls back to OS dictation where present (Windows
//!   `System.Speech`), otherwise returns `None` so the caller asks the user to
//!   type.
//!
//! A missing or failing engine is never fatal: `speak` returns `false` and
//! `listen` returns `None`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Speak `text` aloud with the OS-native TTS engine, blocking until it finishes.
/// Returns `false` (and is silent) when the text is empty or no engine is
/// available — the caller just shows the text instead.
pub fn speak(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    speak_impl(text)
}

/// Capture one spoken utterance as text. Tries the local transcription engine
/// (record + transcribe) first, then OS dictation, else `None` (type instead).
pub fn listen() -> Option<String> {
    whisper_listen().or_else(os_listen)
}

/// Voice transport mode from `FLEETY_VOICE_AUDIO`: `auto` (default), `on`, `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceAudio {
    Auto,
    On,
    Off,
}

/// What to do with a spoken utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceMode {
    /// Send the captured audio to the model (it transcribes).
    SendAudio,
    /// Transcribe on-device and send text (the existing path).
    LocalStt,
}

/// Resolve the voice transport setting from env (unknown values → Auto).
pub fn voice_audio_setting() -> VoiceAudio {
    match std::env::var("FLEETY_VOICE_AUDIO")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "on" => VoiceAudio::On,
        "off" => VoiceAudio::Off,
        _ => VoiceAudio::Auto,
    }
}

/// Decide the voice mode: `on` always sends audio, `off` always transcribes
/// locally, `auto` sends audio only when the model accepts audio input. Pure.
pub fn voice_mode(audio_input: bool, setting: VoiceAudio) -> VoiceMode {
    match setting {
        VoiceAudio::On => VoiceMode::SendAudio,
        VoiceAudio::Off => VoiceMode::LocalStt,
        VoiceAudio::Auto => {
            if audio_input {
                VoiceMode::SendAudio
            } else {
                VoiceMode::LocalStt
            }
        }
    }
}

/// Whether an encoded-audio payload is within the configured size cap. Pure.
pub fn within_limit(len: usize, cap: usize) -> bool {
    len <= cap
}

/// Max audio payload bytes before falling back to local STT
/// (`FLEETY_VOICE_AUDIO_MAX_KB`, default 2048 KB).
fn audio_size_cap() -> usize {
    let kb = std::env::var("FLEETY_VOICE_AUDIO_MAX_KB")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2048);
    kb * 1024
}

/// Capture a spoken utterance as a compact 16 kHz mono WAV attachment
/// `(bytes, mime)`, or `None` when capture fails or the payload exceeds the cap
/// (the caller then falls back to local transcription).
pub fn capture_audio() -> Option<(Vec<u8>, &'static str)> {
    let pcm16 = record_pcm16(stt_seconds())?;
    if pcm16.is_empty() {
        return None;
    }
    let bytes = wav_bytes_mono16(&pcm16);
    if !within_limit(bytes.len(), audio_size_cap()) {
        tracing::warn!(
            bytes = bytes.len(),
            "captured audio exceeds cap; falling back to local STT"
        );
        return None;
    }
    Some((bytes, "audio/wav"))
}

// --- speech-to-text: record (cpal) + transcribe (whisper.cpp) ---------------

/// Seconds to record per utterance (`FLEETY_STT_SECONDS`, default 5).
fn stt_seconds() -> u64 {
    std::env::var("FLEETY_STT_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(5)
}

/// Record the mic, transcribe via the configured engine, return the text. Any
/// missing piece (no device, no engine, empty result) yields `None`. The temp
/// WAV is always removed.
fn whisper_listen() -> Option<String> {
    let wav = record_wav(stt_seconds())?;
    let text = transcribe(&wav);
    let _ = std::fs::remove_file(&wav);
    text
}

/// Build the transcription command. `FLEETY_STT_CMD` is a template where `{wav}`
/// and `{model}` are substituted; with no template it defaults to whisper.cpp
/// (`whisper-cli -m <model> -f <wav> -nt`), which needs `FLEETY_STT_MODEL`.
/// Returns `(program, args)`, or `None` when the default is selected with no
/// model configured.
fn build_transcribe(
    template: Option<&str>,
    model: Option<&str>,
    wav: &str,
) -> Option<(String, Vec<String>)> {
    let filled = match template {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => format!("whisper-cli -m {} -f {wav} -nt", model?),
    };
    let filled = filled
        .replace("{wav}", wav)
        .replace("{model}", model.unwrap_or(""));
    let mut parts = filled.split_whitespace().map(str::to_string);
    let program = parts.next()?;
    Some((program, parts.collect()))
}

/// Transcribe a WAV file to text via the configured engine; `None` on any failure.
fn transcribe(wav: &Path) -> Option<String> {
    let wav_s = wav.to_str()?;
    let template = std::env::var("FLEETY_STT_CMD").ok();
    let model = std::env::var("FLEETY_STT_MODEL").ok();
    let (program, args) = build_transcribe(template.as_deref(), model.as_deref(), wav_s)?;
    let argref: Vec<&str> = args.iter().map(String::as_str).collect();
    run_capture(&program, &argref)
}

/// No-op cpal stream error handler (a stream error just ends recording).
fn record_err(_e: cpal::StreamError) {}

/// Record `seconds` of microphone audio and write it as a 16 kHz mono 16-bit
/// WAV, returning its path. `None` when there is no input device, the format is
/// unsupported, or nothing was captured. Never panics.
fn record_wav(seconds: u64) -> Option<PathBuf> {
    let pcm16 = record_pcm16(seconds)?;
    let path = std::env::temp_dir().join(format!("fleety-stt-{}.wav", std::process::id()));
    write_wav_mono16(&path, &pcm16).ok()?;
    Some(path)
}

/// Record `seconds` of microphone audio as 16 kHz mono 16-bit PCM samples.
/// `None` when there is no input device, the format is unsupported, or nothing
/// was captured. Never panics. Shared by local STT and audio-to-model.
fn record_pcm16(seconds: u64) -> Option<Vec<i16>> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host.default_input_device()?;
    let supported = device.default_input_config().ok()?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let b = Arc::clone(&buf);
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut v) = b.lock() {
                        v.extend_from_slice(data);
                    }
                },
                record_err,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let b = Arc::clone(&buf);
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut v) = b.lock() {
                        v.extend(data.iter().map(|&s| f32::from(s) / 32768.0));
                    }
                },
                record_err,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let b = Arc::clone(&buf);
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut v) = b.lock() {
                        v.extend(data.iter().map(|&s| (f32::from(s) - 32768.0) / 32768.0));
                    }
                },
                record_err,
                None,
            )
        }
        _ => return None,
    }
    .ok()?;

    stream.play().ok()?;
    std::thread::sleep(Duration::from_secs(seconds));
    drop(stream);

    let samples = buf.lock().ok()?.clone();
    if samples.is_empty() {
        return None;
    }
    let mono = downmix(&samples, channels);
    Some(resample_to_16k(&mono, sample_rate))
}

/// Average interleaved channels down to mono.
fn downmix(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Nearest-neighbour resample mono `f32` to 16 kHz `i16` PCM.
fn resample_to_16k(input: &[f32], in_rate: u32) -> Vec<i16> {
    let target = 16_000u32;
    if input.is_empty() {
        return Vec::new();
    }
    if in_rate == target {
        return input.iter().map(|&s| to_i16(s)).collect();
    }
    let out_len = (input.len() as u64 * u64::from(target) / u64::from(in_rate)) as usize;
    (0..out_len)
        .map(|i| {
            let src = (i as u64 * u64::from(in_rate) / u64::from(target)) as usize;
            to_i16(input.get(src).copied().unwrap_or(0.0))
        })
        .collect()
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Build a minimal 16 kHz mono 16-bit PCM WAV in memory.
fn wav_bytes_mono16(samples: &[i16]) -> Vec<u8> {
    let sample_rate: u32 = 16_000;
    let channels: u16 = 1;
    let bits: u16 = 16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits) / 8;
    let block_align = channels * bits / 8;
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Write a minimal 16 kHz mono 16-bit PCM WAV to `path`.
fn write_wav_mono16(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    std::fs::write(path, wav_bytes_mono16(samples))
}

// --- shared command helpers -------------------------------------------------

/// Run a fire-and-forget engine command, returning whether it succeeded. A
/// missing binary or spawn failure yields `false` rather than panicking.
fn run_status(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run an engine command and capture its trimmed stdout. A missing binary,
/// failure, or empty output yields `None`.
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

// --- TTS (OS-native) --------------------------------------------------------

#[cfg(target_os = "macos")]
fn speak_impl(text: &str) -> bool {
    run_status("say", &[text])
}

#[cfg(target_os = "windows")]
fn speak_impl(text: &str) -> bool {
    // SAPI via PowerShell. Single-quote-escape for the PS string literal.
    let escaped = text.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{escaped}')"
    );
    run_status(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn speak_impl(text: &str) -> bool {
    // speech-dispatcher; --wait blocks until the utterance finishes.
    run_status("spd-say", &["--wait", text])
}

#[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
fn speak_impl(_text: &str) -> bool {
    false
}

// --- STT OS-dictation fallback ---------------------------------------------

#[cfg(target_os = "windows")]
fn os_listen() -> Option<String> {
    // System.Speech free-dictation, one utterance. Any failure (no mic, nothing
    // said) surfaces as a non-zero exit or empty stdout → None.
    let script = "Add-Type -AssemblyName System.Speech; \
         $rec = New-Object System.Speech.Recognition.SpeechRecognitionEngine; \
         $rec.SetInputToDefaultAudioDevice(); \
         $rec.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar)); \
         $r = $rec.Recognize(); \
         if ($r) { [Console]::Out.Write($r.Text) }";
    run_capture(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
}

#[cfg(not(target_os = "windows"))]
fn os_listen() -> Option<String> {
    // No reliable headless OS dictation on macOS/Linux: signal type-instead.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_engine_does_not_crash() {
        // A non-existent engine binary must fail gracefully, never panic — this
        // is also the transcription-command-missing fallback path.
        assert!(!run_status("fleety-no-such-tts-binary-xyz", &["hi"]));
        assert!(run_capture("fleety-no-such-stt-binary-xyz", &[]).is_none());
    }

    #[test]
    fn empty_text_is_not_spoken() {
        assert!(!speak(""));
        assert!(!speak("   "));
    }

    #[test]
    fn build_transcribe_default_needs_model() {
        // Default engine without a model can't be built → None (falls back).
        assert!(build_transcribe(None, None, "a.wav").is_none());
        let (program, args) = build_transcribe(None, Some("m.bin"), "a.wav").expect("default");
        assert_eq!(program, "whisper-cli");
        assert!(args.contains(&"a.wav".to_string()) && args.contains(&"m.bin".to_string()));
    }

    #[test]
    fn build_transcribe_substitutes_template_placeholders() {
        let (program, args) =
            build_transcribe(Some("mywhisper {model} {wav}"), Some("m.bin"), "a.wav")
                .expect("template");
        assert_eq!(program, "mywhisper");
        assert_eq!(args, vec!["m.bin".to_string(), "a.wav".to_string()]);
    }

    #[test]
    fn voice_mode_decision_table() {
        // (audio_input, setting) → mode, per the spec example table.
        assert_eq!(voice_mode(true, VoiceAudio::Auto), VoiceMode::SendAudio);
        assert_eq!(voice_mode(false, VoiceAudio::Auto), VoiceMode::LocalStt);
        assert_eq!(voice_mode(false, VoiceAudio::On), VoiceMode::SendAudio);
        assert_eq!(voice_mode(true, VoiceAudio::Off), VoiceMode::LocalStt);
    }

    #[test]
    fn within_limit_bounds() {
        assert!(within_limit(100, 100));
        assert!(within_limit(50, 100));
        assert!(!within_limit(101, 100));
    }

    #[test]
    fn wav_bytes_match_file_writer() {
        let samples = [0i16, 1, -1, 32767, -32768];
        let bytes = wav_bytes_mono16(&samples);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        // 44-byte header + 2 bytes per sample.
        assert_eq!(bytes.len(), 44 + samples.len() * 2);
    }

    #[test]
    fn wav_header_is_well_formed() {
        let dir = std::env::temp_dir().join(format!("fleety-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("t.wav");
        write_wav_mono16(&path, &[0i16, 1, -1, 32767, -32768]).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[36..40], b"data");
        // 44-byte header + 5 samples * 2 bytes.
        assert_eq!(bytes.len(), 44 + 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downmix_and_resample_basics() {
        // Stereo → mono averages pairs.
        assert_eq!(downmix(&[1.0, 1.0, 0.0, 0.0], 2), vec![1.0, 0.0]);
        // Same-rate resample preserves length.
        assert_eq!(resample_to_16k(&[0.0, 0.5], 16_000).len(), 2);
        // Halving the rate's worth of samples downsamples ~by ratio.
        assert_eq!(resample_to_16k(&vec![0.0; 32_000], 32_000).len(), 16_000);
    }
}
