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

---
### Requirement: transfer_file relays a file between two endpoints

The server SHALL provide a `transfer_file` tool that copies a single file from a source endpoint to a destination endpoint, where each endpoint is either a connected device (by `device_id`) or the server itself (`server`). It SHALL read the source bytes (from a device via the existing on-device dispatch of `read_file_bytes`, or from the server via the shared byte helper) and write them to the destination (via `write_file_bytes` on a device, or the shared helper on the server), then compare the source and destination SHA-256: a mismatch SHALL be reported as a corrupted transfer and NOT treated as success. On success it SHALL return the byte count and SHA-256. A destination device that has not advertised `write_file_bytes` (an older daemon) SHALL yield the existing "did not advertise" error, and a disconnected device the existing "not connected" error. The write SHALL back up an existing destination file, so a mismatched transfer is recoverable via rollback.

#### Scenario: device-to-device transfer verifies integrity

- **WHEN** `transfer_file` copies a file from one connected device to another and both SHA-256 values match
- **THEN** it succeeds and returns the byte count and SHA-256

#### Scenario: server is a valid endpoint in either direction

- **WHEN** `transfer_file` uses `server` as the source or the destination
- **THEN** it reads from / writes to the server's workspace via the shared byte helper, transferring to / from the other endpoint

#### Scenario: a corrupted relay is not a success

- **WHEN** the destination SHA-256 does not match the source
- **THEN** the tool reports a corrupted transfer rather than success, and the backed-up destination can be rolled back

<!-- @trace
source: cross-device-file-transfer
updated: 2026-07-12
code:
  - prompts/protocol.md
  - docs/env.md
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - docs/tools.md
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/conn.rs
-->

---
### Requirement: Daemon routing is not displaced by interactive sessions

Only a daemon-capable connection that advertises on-device tools SHALL occupy the routable device sender entry. An interactive CLI connection using the same stable device id SHALL receive its own replies but SHALL NOT replace or remove the daemon sender. Disconnect cleanup SHALL remove a sender only when the entry still belongs to the disconnecting connection.

#### Scenario: CLI connection does not replace daemon

- **GIVEN** fleetyd is connected under device id laptop
- **WHEN** an interactive fleety CLI connects under the same device id
- **THEN** device routing continues to send RunTool frames to fleetyd

#### Scenario: CLI disconnect does not remove daemon

- **GIVEN** fleetyd and an interactive CLI are connected under the same device id
- **WHEN** the CLI disconnects
- **THEN** the fleetyd sender remains routable

#### Scenario: stale daemon disconnect does not remove replacement

- **GIVEN** a newer fleetyd connection replaced an older daemon sender for the same device id
- **WHEN** the older connection finishes cleanup
- **THEN** the newer sender remains registered

<!-- @trace
source: route-config-to-owning-runtime
updated: 2026-07-14
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/provider_tui.rs
  - docs/design-cli-config.md
  - docs/roadmap.md
  - README.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-cli/src/main.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-cli/src/server.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
-->