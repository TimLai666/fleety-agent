# device-deixis Specification

## Purpose

TBD - created by archiving change 'voice-followups'. Update Purpose after archive.

## Requirements

### Requirement: The terminal turn may carry an attention hint

When voice is on, the terminal turn MAY carry a structured attention hint — which device to look at, what to look at, and an optional url or path — alongside the reply. The core SHALL parse this hint from the model's final message the same way it parses the spoken channel (a delimited block), and SHALL leave it empty when the model emits none.

#### Scenario: core parses an attention hint

- **WHEN** the model's final voice-mode message contains an attention block naming a device, a thing to look at, and an optional url
- **THEN** the reply carries a structured hint with those fields

##### Example: parse outcomes

| Model attention block | Parsed hint |
| --------------------- | ----------- |
| `device=lab-pi-a; look=the dashboard; url=http://pi-a/grafana` | device "lab-pi-a", look_at "the dashboard", url set |
| `device=nas; look=the plex transcode log` | device "nas", look_at "the plex transcode log", no url |
| (no attention block) | none |


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
### Requirement: Attention hints are carried backward-compatibly

The attention hint SHALL ride on the assistant reply as an optional field, so a client or server that omits it still interoperates and the protocol version is not bumped. It SHALL be present only on the terminal turn when voice is on.

#### Scenario: old client ignores the hint

- **WHEN** an assistant frame without the attention field is received
- **THEN** it is treated as having no attention hint and the exchange proceeds normally


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
### Requirement: The terminal surfaces the attention hint

When the terminal receives an attention hint, it SHALL surface it to the user — naming the device and what to look at, and offering or opening the url/path when present.

#### Scenario: terminal shows where to look

- **WHEN** the terminal receives a reply carrying an attention hint
- **THEN** it tells the user which device and what to look at, and surfaces the url/path if one is given

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