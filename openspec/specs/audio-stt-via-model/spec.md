# audio-stt-via-model Specification

## Purpose

TBD - created by archiving change 'audio-stt-via-model'. Update Purpose after archive.

## Requirements

### Requirement: The server advertises model audio-input capability to the client

The server SHALL tell the client, as an additive field on the connection handshake (`Welcome`), whether the active model accepts audio input. The field SHALL default to false so an older server that omits it, or an older client that ignores it, behaves as audio-incapable (local STT). The value SHALL come from the provider's modality capability.

#### Scenario: older server omits the field

- **WHEN** a client connects to a server whose `Welcome` carries no audio-input field
- **THEN** the client treats the model as not audio-capable and uses local speech-to-text


<!-- @trace
source: audio-stt-via-model
updated: 2026-06-29
code:
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/gemini.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/effort.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/model.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-workflow/src/lib.rs
-->

---
### Requirement: Voice uses model audio when supported, else local STT

When the model accepts audio input and the voice-audio setting is not `off`, `fleety voice` SHALL send the captured speech to the model as a compressed audio attachment and SHALL NOT invoke local Whisper. When the model does not accept audio, the setting is `off`, or capability is unknown/offline, it SHALL fall back to the existing on-device transcription that sends text. The decision SHALL be a pure function of (audio-input capability, setting).

#### Scenario: audio-capable model receives compressed audio

- **WHEN** the model is audio-capable and the setting is `auto`
- **THEN** the spoken audio is sent as a compressed audio attachment and no local Whisper transcription runs

#### Scenario: text-only model falls back to local STT

- **WHEN** the model is not audio-capable
- **THEN** voice transcribes on-device and sends text, exactly as before this change

##### Example: voice-mode decision

| audio_input | setting | decision |
|---|---|---|
| true | auto | send audio |
| false | auto | local STT |
| false | on | send audio |
| true | off | local STT |


<!-- @trace
source: audio-stt-via-model
updated: 2026-06-29
code:
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/gemini.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/effort.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/model.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-workflow/src/lib.rs
-->

---
### Requirement: Sent audio is a compact mono encoding and size-bounded

Audio sent to the model SHALL be a compact 16 kHz mono encoding (downmixed and downsampled from the device's native capture) rather than the raw multi-channel/native-rate stream, and SHALL be bounded by a configurable size limit. When the limit would be exceeded, the client SHALL fall back to local transcription rather than send an oversized payload. (A smaller speech codec such as Opus is a follow-up — see the design's open questions.)

#### Scenario: oversized capture falls back

- **WHEN** the encoded audio would exceed the configured size limit
- **THEN** the client falls back to local speech-to-text instead of sending the audio


<!-- @trace
source: audio-stt-via-model
updated: 2026-06-29
code:
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/gemini.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/effort.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/model.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-workflow/src/lib.rs
-->

---
### Requirement: Voice transport mode is configurable

`FLEETY_VOICE_AUDIO` SHALL select the voice transport: `auto` (use the model's audio capability, the default), `on` (always send audio), or `off` (always transcribe locally). An unrecognized value SHALL be treated as `auto`.

#### Scenario: off forces local STT

- **WHEN** `FLEETY_VOICE_AUDIO=off`
- **THEN** voice always transcribes on-device and never sends an audio attachment, regardless of model capability

<!-- @trace
source: audio-stt-via-model
updated: 2026-06-29
code:
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/gemini.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/effort.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/model.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-workflow/src/lib.rs
-->