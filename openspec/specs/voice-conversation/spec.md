# voice-conversation Specification

## Purpose

TBD - created by archiving change 'voice-conversation'. Update Purpose after archive.

## Requirements

### Requirement: Per-message voice flag

The system SHALL carry a per-message `voice` flag from the client to the core. When the flag is off, the system SHALL NOT request or produce any spoken output and SHALL NOT spend extra tokens on it. The flag SHALL default to off when absent.

#### Scenario: voice off is a normal text turn

- **WHEN** a user message arrives with the voice flag off (or absent)
- **THEN** the turn runs exactly as a non-voice turn, producing no speech and adding no spoken-output instruction to the model

#### Scenario: voice on requests spoken output

- **WHEN** a user message arrives with the voice flag on
- **THEN** the core's system prompt includes the dual-channel instruction so the model can produce a spoken version


<!-- @trace
source: voice-conversation
updated: 2026-06-28
code:
  - prompts/protocol.md
  - docs/spec-v0.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/subagent.rs
-->

---
### Requirement: Dual-channel output on the terminal turn

When voice is on, the terminal turn SHALL produce two channels from a single model call: a display text and a spoken version. The core SHALL split the model's final message into display and speech at an agreed sentinel: text before the sentinel is the display, text after it is the speech. When the sentinel is absent, the speech SHALL be empty and the display SHALL be the whole message, without error.

#### Scenario: core splits display and speech at the sentinel

- **WHEN** the model's final message contains display text followed by the speech sentinel and a spoken version
- **THEN** the display is the text before the sentinel and the speech is the text after it

##### Example: split outcomes

| Model final message | Display | Speech |
| ------------------- | ------- | ------ |
| `Here is the diff.\n⟦SPEECH⟧\nDone — check the diff.` | `Here is the diff.` | `Done — check the diff.` |
| `All set, no sentinel here.` | `All set, no sentinel here.` | (none) |


<!-- @trace
source: voice-conversation
updated: 2026-06-28
code:
  - prompts/protocol.md
  - docs/spec-v0.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/subagent.rs
-->

---
### Requirement: Only the terminal turn speaks

Intermediate continuation turns of a goal loop SHALL NOT emit speech to the user. Only the terminal turn — the one that calls `complete_goal` or `ask_user`, or a single-shot turn — SHALL carry the spoken version in its user-facing reply.

#### Scenario: speech rides only the terminal reply

- **WHEN** a voice-on goal loop runs several intermediate continuation turns and then completes
- **THEN** only the final assistant reply carries speech, and no intermediate reply carries speech


<!-- @trace
source: voice-conversation
updated: 2026-06-28
code:
  - prompts/protocol.md
  - docs/spec-v0.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/subagent.rs
-->

---
### Requirement: Backward-compatible protocol

The new `voice` and `speech` wire fields SHALL be optional so that a client or server that omits them still interoperates, and the protocol version SHALL NOT be bumped for this change.

#### Scenario: old and new peers interoperate

- **WHEN** a message without the `voice` field is received, or an assistant frame without the `speech` field is received
- **THEN** the missing `voice` is treated as off and the missing `speech` is treated as none, and the exchange proceeds normally

##### Example: wire compatibility

| Wire payload | Interpreted as |
| ------------ | -------------- |
| user message with no `voice` field | voice off |
| assistant frame with no `speech` field | no spoken output |
| user message with `voice: true` | voice on |


<!-- @trace
source: voice-conversation
updated: 2026-06-28
code:
  - prompts/protocol.md
  - docs/spec-v0.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/subagent.rs
-->

---
### Requirement: Terminal OS-native speech with graceful fallback

The terminal SHALL use the operating system's native engines for speech: text-to-speech to read the spoken version aloud, and speech-to-text to turn spoken input into a user message with the voice flag on. When an engine is missing or fails, the terminal SHALL fall back to plain text and SHALL NOT crash. Where the operating system provides no native speech-to-text, the terminal SHALL report that dictation is unavailable and ask the user to type instead.

#### Scenario: missing engine falls back to text

- **WHEN** the OS speech engine is unavailable or returns an error
- **THEN** the terminal continues in plain text — text-to-speech is skipped and speech-to-text prompts the user to type — without crashing

#### Scenario: spoken reply is read aloud

- **WHEN** the terminal receives an assistant reply that carries a spoken version while voice is on
- **THEN** the terminal displays the text and reads the spoken version aloud with the OS engine

<!-- @trace
source: voice-conversation
updated: 2026-06-28
code:
  - prompts/protocol.md
  - docs/spec-v0.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/subagent.rs
-->

---
### Requirement: Voice-activity endpointed capture

The terminal SHALL capture a spoken utterance using voice-activity detection: recording SHALL begin listening immediately, SHALL treat sustained microphone energy above a configured threshold as speech, and SHALL end the utterance once trailing silence exceeds a configured hangover duration. The capture SHALL enforce a maximum-utterance duration cap and a start-of-speech timeout; on reaching either cap it SHALL return whatever was captured, or none when no speech was ever detected, without crashing. When voice-activity detection is disabled by configuration, the terminal SHALL fall back to fixed-duration recording.

#### Scenario: silence ends the utterance

- **WHEN** a user speaks and then stops for longer than the configured trailing-silence hangover
- **THEN** recording ends and the captured audio up to that point is returned for transcription or upload

#### Scenario: maximum-duration cap stops a long utterance

- **WHEN** a user keeps speaking past the configured maximum utterance duration
- **THEN** recording stops at the cap and returns the captured audio without error

#### Scenario: no speech within the start timeout returns nothing

- **WHEN** no microphone energy above the threshold is detected within the configured start-of-speech timeout
- **THEN** capture ends and returns none, so the caller prompts the user to type instead, without crashing

#### Scenario: disabling voice-activity detection restores fixed-duration recording

- **WHEN** voice-activity detection is turned off by configuration
- **THEN** the terminal records for the fixed configured number of seconds exactly as before


<!-- @trace
source: voice-vad-barge-in
updated: 2026-07-10
code:
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/identity.rs
  - Dockerfile
  - crates/fleety-cli/src/main.rs
  - docs/env.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-daemon/src/service.rs
  - scripts/install.sh
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/provider_tui.rs
  - README.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Barge-in during spoken playback

While the terminal is reading a spoken reply aloud, it SHALL monitor the microphone for the onset of user speech and, on detecting it, SHALL stop playback immediately and proceed to capture the user's utterance. The onset test SHALL require sustained energy across several consecutive analysis windows so that transient noise and speaker echo do not falsely interrupt playback. Barge-in SHALL be configurable on or off; when off, or when no microphone or speech engine is available, the terminal SHALL play the reply to completion without crashing.

#### Scenario: user speech interrupts playback

- **WHEN** the user begins speaking while a spoken reply is being read aloud and barge-in is enabled
- **THEN** playback stops immediately and the terminal proceeds to capture the user's next utterance

#### Scenario: uninterrupted playback runs to completion

- **WHEN** no sustained user speech is detected while a spoken reply is read aloud
- **THEN** the reply is read to completion and the terminal then listens for the next utterance

#### Scenario: barge-in disabled or unavailable never interrupts

- **WHEN** barge-in is turned off by configuration, or no microphone or speech engine is available
- **THEN** the reply is played to completion before the terminal listens again, without crashing

<!-- @trace
source: voice-vad-barge-in
updated: 2026-07-10
code:
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/identity.rs
  - Dockerfile
  - crates/fleety-cli/src/main.rs
  - docs/env.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-daemon/src/service.rs
  - scripts/install.sh
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/provider_tui.rs
  - README.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->