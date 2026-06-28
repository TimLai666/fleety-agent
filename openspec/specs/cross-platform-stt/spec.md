# cross-platform-stt Specification

## Purpose

TBD - created by archiving change 'voice-followups'. Update Purpose after archive.

## Requirements

### Requirement: Terminal transcribes speech via a configurable engine

The terminal SHALL turn spoken input into a user message (with the voice flag on) by recording the microphone and transcribing it through a configurable command, defaulting to a local engine. The server SHALL continue to exchange only text — recording and transcription happen entirely on the terminal.

#### Scenario: speech becomes a voice message

- **WHEN** voice input is requested and a transcription command is available
- **THEN** the terminal records the microphone, transcribes it to text, and sends that text as a user message with the voice flag on


<!-- @trace
source: voice-followups
updated: 2026-06-28
code:
  - crates/agent-core/src/agent.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/Cargo.toml
  - prompts/protocol.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->

---
### Requirement: Speech-to-text degrades gracefully

When no transcription command, microphone, or model is available, or any step fails, the terminal SHALL fall back to plain text and SHALL NOT crash. Where the operating system offers its own dictation, that fallback MAY be used; otherwise the terminal SHALL ask the user to type.

#### Scenario: missing engine falls back without crashing

- **WHEN** the transcription command or microphone is unavailable, or transcription returns nothing
- **THEN** the terminal does not crash and prompts the user to type (or uses the OS dictation fallback where present)

##### Example: input outcomes

| Situation | Result |
| --------- | ------ |
| transcription command present, speech recognized | voice message sent with the transcript |
| command missing / mic unavailable / empty transcript | fall back to typing (no crash) |
| transcription command exits non-zero | fall back to typing (no crash) |

<!-- @trace
source: voice-followups
updated: 2026-06-28
code:
  - crates/agent-core/src/agent.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/Cargo.toml
  - prompts/protocol.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->