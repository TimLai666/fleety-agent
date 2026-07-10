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
//! ## Endpointing and barge-in
//!
//! Capture uses an energy-threshold voice-activity detector (VAD): it listens
//! immediately, treats sustained microphone energy as speech, and ends the
//! utterance on trailing silence, with hard caps on total length and on how long
//! to wait for speech to start. While a reply is being read aloud the mic is
//! watched for the onset of user speech and playback is killed on barge-in. All
//! knobs fall back to the listed default when unset or invalid:
//!
//! - `FLEETY_VAD` (`on`/`off`, default `on`): `off` restores fixed-duration
//!   recording controlled by `FLEETY_STT_SECONDS`.
//! - `FLEETY_VAD_ENERGY` (RMS threshold for "hot" windows, default `0.02`).
//! - `FLEETY_VAD_SILENCE_MS` (trailing-silence hangover before ending, default `800`).
//! - `FLEETY_VAD_MAX_MS` (maximum utterance length, default `15000`).
//! - `FLEETY_VAD_START_TIMEOUT_MS` (give up if no speech starts, default `8000`).
//! - `FLEETY_BARGE_IN` (`on`/`off`, default `on`): stop spoken playback when the
//!   user starts talking over it.
//!
//! A missing or failing engine is never fatal: `speak` returns `false` and
//! `listen` returns `None`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Speak `text` aloud with the OS-native TTS engine, blocking until it finishes.
/// Returns `false` (and is silent) when the text is empty or no engine is
/// available — the caller just shows the text instead. Thin wrapper over
/// [`speak_interruptible`]: both `Completed` and `Interrupted` count as "spoken".
/// Retained as a back-compat convenience; the voice loop calls
/// [`speak_interruptible`] directly to observe barge-in.
#[cfg_attr(not(test), allow(dead_code))]
pub fn speak(text: &str) -> bool {
    matches!(
        speak_interruptible(text),
        SpeakOutcome::Completed | SpeakOutcome::Interrupted
    )
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
    let secs = stt_seconds();
    // Tell the user we're listening — without this they have no idea when to
    // speak or why it stopped.
    announce_listening(secs);
    let pcm16 = record_pcm16(secs)?;
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

// --- voice-activity detection (pure) ---------------------------------------

/// Analysis-window length fed to the VAD state machine, in milliseconds.
const VAD_WINDOW_MS: u64 = 30;
/// Consecutive hot windows required to treat playback barge-in as real speech.
const ONSET_WINDOWS: u32 = 5;

/// Root-mean-square energy of a window of normalised (`-1.0..=1.0`) samples.
/// An empty window has zero energy. Pure.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Tunable thresholds for [`VadState`], resolved from env by
/// [`vad_config_from_env`].
#[derive(Debug, Clone, Copy)]
struct VadConfig {
    /// RMS above which a window counts as speech.
    energy: f32,
    /// Trailing silence that ends an utterance, in ms.
    silence_ms: u64,
    /// Hard cap on a single utterance, in ms.
    max_ms: u64,
    /// Give up if no speech starts within this many ms.
    start_timeout_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            energy: 0.02,
            silence_ms: 800,
            max_ms: 15_000,
            start_timeout_ms: 8_000,
        }
    }
}

/// Why the VAD state machine ended capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    /// Trailing silence exceeded the hangover after speech.
    Silence,
    /// The maximum-utterance cap was reached.
    MaxDuration,
    /// No speech was ever detected within the start timeout.
    StartTimeout,
}

/// Per-window decision from [`VadState::observe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VadDecision {
    /// No speech yet; keep listening.
    WaitingForSpeech,
    /// Speech is in progress; keep recording.
    Speaking,
    /// Stop recording for the given reason.
    Stop(EndReason),
}

/// Endpointing state machine. Feed it one window's RMS at a time via
/// [`observe`](VadState::observe); it tracks whether speech has started, the
/// accumulated trailing silence, and the total elapsed time. Pure (no I/O).
#[derive(Debug, Clone)]
struct VadState {
    config: VadConfig,
    speech_started: bool,
    silence_ms: u64,
    total_ms: u64,
}

impl VadState {
    fn new(config: VadConfig) -> Self {
        Self {
            config,
            speech_started: false,
            silence_ms: 0,
            total_ms: 0,
        }
    }

    /// Advance by one `window_ms` window whose energy is `window_rms`.
    fn observe(&mut self, window_rms: f32, window_ms: u64) -> VadDecision {
        self.total_ms = self.total_ms.saturating_add(window_ms);
        let hot = window_rms >= self.config.energy;
        if !self.speech_started {
            if hot {
                self.speech_started = true;
                self.silence_ms = 0;
            } else if self.total_ms >= self.config.start_timeout_ms {
                return VadDecision::Stop(EndReason::StartTimeout);
            } else {
                return VadDecision::WaitingForSpeech;
            }
        } else if hot {
            self.silence_ms = 0;
        } else {
            self.silence_ms = self.silence_ms.saturating_add(window_ms);
        }
        // Speech has started (this window or earlier). End on trailing silence
        // first, then on the hard length cap.
        if self.silence_ms >= self.config.silence_ms {
            return VadDecision::Stop(EndReason::Silence);
        }
        if self.total_ms >= self.config.max_ms {
            return VadDecision::Stop(EndReason::MaxDuration);
        }
        VadDecision::Speaking
    }
}

/// Whether `consecutive_hot` above-threshold windows meet the sustained-energy
/// bar for a barge-in onset. Pure.
fn onset_reached(consecutive_hot: u32, needed: u32) -> bool {
    needed > 0 && consecutive_hot >= needed
}

/// Parse a positive `u64` env var, falling back to `default` on unset/invalid
/// (mirrors [`stt_seconds`]).
fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// Parse a positive, finite `f32` env var, falling back to `default`.
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|&n| n > 0.0 && n.is_finite())
        .unwrap_or(default)
}

/// Resolve [`VadConfig`] from the `FLEETY_VAD_*` env vars (invalid → default).
fn vad_config_from_env() -> VadConfig {
    let d = VadConfig::default();
    VadConfig {
        energy: env_f32("FLEETY_VAD_ENERGY", d.energy),
        silence_ms: env_u64("FLEETY_VAD_SILENCE_MS", d.silence_ms),
        max_ms: env_u64("FLEETY_VAD_MAX_MS", d.max_ms),
        start_timeout_ms: env_u64("FLEETY_VAD_START_TIMEOUT_MS", d.start_timeout_ms),
    }
}

/// VAD endpointing on unless `FLEETY_VAD=off`.
fn vad_enabled() -> bool {
    !std::env::var("FLEETY_VAD")
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("off")
}

/// Barge-in on unless `FLEETY_BARGE_IN=off`.
fn barge_in_enabled() -> bool {
    !std::env::var("FLEETY_BARGE_IN")
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("off")
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

/// Print the "we're listening" prompt. VAD mode self-endpoints on a pause; the
/// fixed fallback (`FLEETY_VAD=off`) records `secs` seconds as before.
fn announce_listening(secs: u64) {
    if vad_enabled() {
        eprintln!("● listening — speak now; pause when done (FLEETY_VAD=off for fixed {secs}s)");
    } else {
        eprintln!("● recording {secs}s — speak now (FLEETY_STT_SECONDS adjusts)");
    }
}

/// Record the mic, transcribe via the configured engine, return the text. Any
/// missing piece (no device, no engine, empty result) yields `None`. The temp
/// WAV is always removed.
fn whisper_listen() -> Option<String> {
    let secs = stt_seconds();
    announce_listening(secs);
    let wav = record_wav(secs)?;
    eprintln!("… transcribing");
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

/// A live microphone stream plus the shared buffer its callback appends
/// normalised (`-1.0..=1.0`) interleaved samples to. Dropping `stream` stops
/// capture. Shared by fixed-duration recording, VAD capture, and barge-in.
struct MicStream {
    stream: cpal::Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    channels: usize,
}

/// Open and start the default input device, normalising every supported sample
/// format to `f32`. `None` when there is no device, the format is unsupported,
/// or the stream can't be built/started. Never panics.
fn open_mic() -> Option<MicStream> {
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
    Some(MicStream {
        stream,
        buf,
        sample_rate,
        channels,
    })
}

/// Record one utterance as 16 kHz mono 16-bit PCM samples. With VAD on
/// (default) it listens until the endpointer stops; with `FLEETY_VAD=off` it
/// records `seconds` seconds exactly as before. `None` when there is no input
/// device, the format is unsupported, no speech was ever detected, or nothing
/// was captured. Never panics. Shared by local STT and audio-to-model.
fn record_pcm16(seconds: u64) -> Option<Vec<i16>> {
    let MicStream {
        stream,
        buf,
        sample_rate,
        channels,
    } = open_mic()?;

    if !vad_enabled() {
        // Fixed-duration fallback: record for the configured seconds, then take
        // the whole buffer — identical to the pre-VAD behaviour.
        std::thread::sleep(Duration::from_secs(seconds));
        drop(stream);
        return finish_capture(&buf, channels, sample_rate);
    }

    // VAD-driven capture: feed one window of energy to the state machine per
    // wall-clock tick. Reading "all new samples since last tick" keeps the VAD
    // clock aligned to real time, so the start-timeout and max-duration caps
    // still fire even if the mic stalls and delivers no callbacks.
    let mut state = VadState::new(vad_config_from_env());
    let mut processed = 0usize;
    let stop_reason = loop {
        std::thread::sleep(Duration::from_millis(VAD_WINDOW_MS));
        let window_rms = match buf.lock() {
            Ok(guard) => {
                let len = guard.len();
                let value = if len > processed {
                    rms(&downmix(&guard[processed..len], channels))
                } else {
                    0.0
                };
                processed = len;
                value
            }
            // Poisoned lock: stop and return whatever we have.
            Err(_) => break EndReason::Silence,
        };
        if let VadDecision::Stop(reason) = state.observe(window_rms, VAD_WINDOW_MS) {
            break reason;
        }
    };
    drop(stream);

    // Timed out before any speech → nothing to transcribe.
    if matches!(stop_reason, EndReason::StartTimeout) {
        return None;
    }
    finish_capture(&buf, channels, sample_rate)
}

/// Downmix and resample the captured buffer to 16 kHz mono `i16`; `None` when
/// empty. Shared tail of both capture paths.
fn finish_capture(
    buf: &Arc<Mutex<Vec<f32>>>,
    channels: usize,
    sample_rate: u32,
) -> Option<Vec<i16>> {
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

/// Outcome of [`speak_interruptible`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakOutcome {
    /// The engine finished reading the whole reply.
    Completed,
    /// The user barged in; playback was stopped early.
    Interrupted,
    /// Nothing was spoken — empty text or no available engine.
    Unavailable,
}

/// Speak `text` aloud, but stop as soon as the user starts talking over it
/// (barge-in). Spawns the OS TTS engine as a child process and, while it runs,
/// watches the microphone; `ONSET_WINDOWS` consecutive above-threshold windows
/// count as user speech and kill playback (`Interrupted`). Empty text or a
/// missing engine yields `Unavailable`; with `FLEETY_BARGE_IN=off` or no usable
/// mic it plays to completion (`Completed`). Never panics: spawn/kill failures
/// degrade to blocking playback.
pub fn speak_interruptible(text: &str) -> SpeakOutcome {
    let text = text.trim();
    if text.is_empty() {
        return SpeakOutcome::Unavailable;
    }
    let mut child = match spawn_tts(text) {
        Some(child) => child,
        None => return SpeakOutcome::Unavailable,
    };

    // No barge-in, or no mic to watch: just wait for the engine to finish.
    if !barge_in_enabled() {
        let _ = child.wait();
        return SpeakOutcome::Completed;
    }
    let mic = match open_mic() {
        Some(mic) => mic,
        None => {
            let _ = child.wait();
            return SpeakOutcome::Completed;
        }
    };
    let MicStream {
        stream,
        buf,
        channels,
        ..
    } = mic;

    let threshold = vad_config_from_env().energy;
    let mut processed = 0usize;
    let mut consecutive_hot = 0u32;
    let outcome = loop {
        // Did the engine finish on its own?
        match child.try_wait() {
            Ok(Some(_)) => break SpeakOutcome::Completed,
            Ok(None) => {}
            // Can't track the child → treat as done rather than spin.
            Err(_) => break SpeakOutcome::Completed,
        }
        std::thread::sleep(Duration::from_millis(VAD_WINDOW_MS));
        let window_rms = match buf.lock() {
            Ok(guard) => {
                let len = guard.len();
                let value = if len > processed {
                    rms(&downmix(&guard[processed..len], channels))
                } else {
                    0.0
                };
                processed = len;
                value
            }
            Err(_) => break SpeakOutcome::Completed,
        };
        consecutive_hot = if window_rms >= threshold {
            consecutive_hot.saturating_add(1)
        } else {
            0
        };
        if onset_reached(consecutive_hot, ONSET_WINDOWS) {
            let _ = child.kill();
            let _ = child.wait();
            break SpeakOutcome::Interrupted;
        }
    };
    drop(stream);
    outcome
}

/// Spawn the OS-native TTS engine as a child process reading `text`. `None` when
/// there is no engine on this platform or the spawn fails. Never panics.
#[cfg(target_os = "macos")]
fn spawn_tts(text: &str) -> Option<std::process::Child> {
    Command::new("say").arg(text).spawn().ok()
}

#[cfg(target_os = "windows")]
fn spawn_tts(text: &str) -> Option<std::process::Child> {
    // SAPI via PowerShell. Single-quote-escape for the PS string literal.
    let escaped = text.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{escaped}')"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .spawn()
        .ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_tts(text: &str) -> Option<std::process::Child> {
    // speech-dispatcher; --wait keeps the child alive until the utterance ends.
    Command::new("spd-say").args(["--wait", text]).spawn().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
fn spawn_tts(_text: &str) -> Option<std::process::Child> {
    None
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
        // A missing transcription binary must fail gracefully (None), never
        // panic — this is the transcription-command-missing fallback path.
        assert!(run_capture("fleety-no-such-stt-binary-xyz", &[]).is_none());
        // Spawning a non-existent TTS engine yields an error we swallow (`.ok()`
        // in `spawn_tts`), not a panic.
        assert!(Command::new("fleety-no-such-tts-binary-xyz")
            .arg("hi")
            .spawn()
            .is_err());
    }

    #[test]
    fn empty_text_is_not_spoken() {
        assert!(!speak(""));
        assert!(!speak("   "));
    }

    #[test]
    fn speak_interruptible_empty_text_returns_unavailable_or_completed() {
        // Empty/whitespace text must not spawn an engine; it degrades to a
        // no-op outcome rather than reading anything aloud.
        assert!(matches!(
            speak_interruptible(""),
            SpeakOutcome::Unavailable | SpeakOutcome::Completed
        ));
        assert!(matches!(
            speak_interruptible("   "),
            SpeakOutcome::Unavailable | SpeakOutcome::Completed
        ));
    }

    #[test]
    fn rms_silence_vs_full_scale() {
        // A silent window has ~zero energy; a full-scale square wave has RMS ~1.
        assert!(rms(&[0.0; 128]) < 1e-6);
        let full = rms(&[1.0, -1.0, 1.0, -1.0]);
        assert!((full - 1.0).abs() < 1e-6);
        // Louder input has strictly higher RMS than quiet input.
        assert!(rms(&[0.5; 64]) > rms(&[0.01; 64]));
        // Empty window is defined as zero energy.
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn vad_endpoints_on_silence() {
        // After speech starts, trailing silence past the hangover ends capture.
        let cfg = VadConfig {
            energy: 0.1,
            silence_ms: 300,
            max_ms: 100_000,
            start_timeout_ms: 100_000,
        };
        let mut st = VadState::new(cfg);
        assert_eq!(st.observe(0.5, 100), VadDecision::Speaking); // speech starts
        assert_eq!(st.observe(0.5, 100), VadDecision::Speaking);
        assert_eq!(st.observe(0.0, 100), VadDecision::Speaking); // silence 100
        assert_eq!(st.observe(0.0, 100), VadDecision::Speaking); // silence 200
        assert_eq!(
            st.observe(0.0, 100),
            VadDecision::Stop(EndReason::Silence) // silence 300 ≥ hangover
        );
    }

    #[test]
    fn vad_stops_at_max_duration() {
        // Continuous speech past the cap stops with MaxDuration, not Silence.
        let cfg = VadConfig {
            energy: 0.1,
            silence_ms: 10_000,
            max_ms: 250,
            start_timeout_ms: 10_000,
        };
        let mut st = VadState::new(cfg);
        assert_eq!(st.observe(0.5, 100), VadDecision::Speaking); // total 100
        assert_eq!(st.observe(0.5, 100), VadDecision::Speaking); // total 200
        assert_eq!(
            st.observe(0.5, 100),
            VadDecision::Stop(EndReason::MaxDuration) // total 300 ≥ 250
        );
    }

    #[test]
    fn vad_start_timeout_without_speech() {
        // Never detecting speech ends with StartTimeout (caller types instead).
        let cfg = VadConfig {
            energy: 0.1,
            silence_ms: 10_000,
            max_ms: 10_000,
            start_timeout_ms: 250,
        };
        let mut st = VadState::new(cfg);
        assert_eq!(st.observe(0.0, 100), VadDecision::WaitingForSpeech); // total 100
        assert_eq!(st.observe(0.01, 100), VadDecision::WaitingForSpeech); // sub-threshold, 200
        assert_eq!(
            st.observe(0.0, 100),
            VadDecision::Stop(EndReason::StartTimeout) // total 300 ≥ 250
        );
    }

    #[test]
    fn onset_requires_sustained_energy() {
        // Barge-in needs `needed` consecutive hot windows; fewer never triggers.
        assert!(!onset_reached(0, 3));
        assert!(!onset_reached(2, 3));
        assert!(onset_reached(3, 3));
        assert!(onset_reached(5, 3));
        // A zero requirement never fires (guards against instant false onset).
        assert!(!onset_reached(0, 0));
    }

    #[test]
    fn vad_config_env_defaults_and_overrides() {
        let keys = [
            "FLEETY_VAD_ENERGY",
            "FLEETY_VAD_SILENCE_MS",
            "FLEETY_VAD_MAX_MS",
            "FLEETY_VAD_START_TIMEOUT_MS",
        ];
        for k in keys {
            std::env::remove_var(k);
        }
        // Unset → documented defaults.
        let d = vad_config_from_env();
        assert!((d.energy - 0.02).abs() < 1e-6);
        assert_eq!(d.silence_ms, 800);
        assert_eq!(d.max_ms, 15_000);
        assert_eq!(d.start_timeout_ms, 8_000);
        // Valid overrides are honoured.
        std::env::set_var("FLEETY_VAD_ENERGY", "0.05");
        std::env::set_var("FLEETY_VAD_SILENCE_MS", "500");
        std::env::set_var("FLEETY_VAD_MAX_MS", "9000");
        std::env::set_var("FLEETY_VAD_START_TIMEOUT_MS", "4000");
        let o = vad_config_from_env();
        assert!((o.energy - 0.05).abs() < 1e-6);
        assert_eq!(o.silence_ms, 500);
        assert_eq!(o.max_ms, 9000);
        assert_eq!(o.start_timeout_ms, 4000);
        // Invalid values fall back to the default (mirrors `stt_seconds`).
        std::env::set_var("FLEETY_VAD_MAX_MS", "not-a-number");
        std::env::set_var("FLEETY_VAD_SILENCE_MS", "0");
        let f = vad_config_from_env();
        assert_eq!(f.max_ms, 15_000);
        assert_eq!(f.silence_ms, 800);
        for k in keys {
            std::env::remove_var(k);
        }
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
