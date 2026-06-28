//! OS-native speech for voice mode: text-to-speech (`speak`) and best-effort
//! speech-to-text (`listen`).
//!
//! Everything here degrades to plain text and never panics. The server only ever
//! exchanges text — STT/TTS happen entirely on this terminal via the operating
//! system's own engines:
//!
//! - **TTS** (`speak`): macOS `say`, Windows SAPI (via PowerShell), Linux
//!   `spd-say` (speech-dispatcher).
//! - **STT** (`listen`): best-effort on Windows (`System.Speech` dictation).
//!   macOS and Linux have no reliable headless dictation CLI, so `listen`
//!   returns `None` there and the caller asks the user to type instead.
//!
//! A missing or failing engine is never fatal: `speak` returns `false` and
//! `listen` returns `None`, and the caller continues in plain text.

use std::process::Command;

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

/// Capture one spoken utterance as text via OS-native STT. Returns `None` when no
/// engine is available or it fails (no microphone, nothing recognized) — the
/// caller should ask the user to type instead.
pub fn listen() -> Option<String> {
    listen_impl()
}

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
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
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

#[cfg(target_os = "windows")]
fn listen_impl() -> Option<String> {
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
fn listen_impl() -> Option<String> {
    // No reliable headless OS dictation on macOS/Linux: signal type-instead.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_engine_does_not_crash() {
        // A non-existent engine binary must fail gracefully, never panic.
        assert!(!run_status("fleety-no-such-tts-binary-xyz", &["hi"]));
        assert!(run_capture("fleety-no-such-stt-binary-xyz", &[]).is_none());
    }

    #[test]
    fn empty_text_is_not_spoken() {
        assert!(!speak(""));
        assert!(!speak("   "));
    }
}
