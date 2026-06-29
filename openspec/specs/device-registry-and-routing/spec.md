# device-registry-and-routing Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Register devices and sites

The system SHALL provide `device_list`, `device_show`, `device_set_site`, `device_set_mobility`, `site_set`, `site_list`, `site_show`, `site_delete`, and `pair_create`. `device_show` SHALL return the device record, its `NOTES`, and the tools it advertised when it last connected. `pair_create` SHALL mint a short-lived pairing code to enroll a new device.

#### Scenario: show a device's advertised tools

- **WHEN** `device_show` is called for a device that advertised its tools at connect time
- **THEN** the result includes that device's record, NOTES, and advertised tool list


<!-- @trace
source: baseline-tool-surface-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - CLAUDE.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-discuss.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-ask.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-drift/SKILL.md
-->

---
### Requirement: Route a tool call to another device

The system SHALL provide `device_exec` that runs a named tool on a connected device by dispatching a `RunTool` frame to that device's daemon and awaiting the reply. When the target device advertised its tools, `device_exec` SHALL strict-check the requested `tool` against that advertised list. Handles a device returns (sessions, pids, ports) SHALL be bound to that device and SHALL be rejected if used against another device.

#### Scenario: reject a foreign handle

- **WHEN** a handle returned by device A is used in a `device_exec` call targeting device B
- **THEN** the call is rejected with an actionable error naming the owning device

<!-- @trace
source: baseline-tool-surface-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - CLAUDE.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-discuss.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-ask.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-drift/SKILL.md
-->

---
### Requirement: Devices are registered under a stable machine-derived id

Device registration and routing SHALL key on a stable, machine-derived device id
(unique across machines, identical for every process on one machine), not a
client-asserted hostname. When a connection is authenticated, the id used for
registration and routing SHALL be the one bound to the authenticated token, so a
client cannot register or be routed to under another device's id. The hostname is
kept only as a display label on the device record.

#### Scenario: routing targets the right machine

- **WHEN** two same-hostname machines are connected
- **THEN** each has a distinct registered id, so a tool routed to one machine does not reach the other

#### Scenario: registration id is authenticated

- **WHEN** an authenticated device registers or is routed to
- **THEN** the id is taken from its token, not from a wire-asserted value

<!-- @trace
source: stable-device-identity
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/device.rs
  - Cargo.toml
  - docs/env.md
  - crates/fleety-server/src/auth.rs
-->