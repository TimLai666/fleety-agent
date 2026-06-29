# device-enrollment Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: Daemon connection configuration

The daemon SHALL read `FLEETY_AGENT_URL` for the server URL, trying mDNS for 2 seconds before falling back to `ws://127.0.0.1:8787`. From the resolved server host the daemon SHALL derive both the WebSocket endpoint and the HTTP(S) endpoints used by the SSE+POST fallback, so that one configured host serves both transports. The daemon SHALL read a setting to force the SSE+POST transport and a setting to disable the SSE fallback; when neither is set, it tries WebSocket first and falls back to SSE. It SHALL read `FLEETY_DEVICE_ID` for this device's id (default the hostname, falling back to `fleetyd-device`; the value is used verbatim and is not sanitized, so a path-safe id is the operator's responsibility) and `FLEETY_DEVICE_ROOT` for the filesystem root its on-device tools operate within (default the current working directory).

#### Scenario: URL falls back to localhost

- **WHEN** `FLEETY_AGENT_URL` is unset and mDNS finds nothing within 2 seconds
- **THEN** the daemon connects to `ws://127.0.0.1:8787`

#### Scenario: SSE endpoint derived from the same host

- **WHEN** the daemon has resolved a server host and the WebSocket transport is unavailable
- **THEN** it reaches the SSE and POST endpoints on that same host without requiring a separately configured URL


<!-- @trace
source: sse-transport-fallback
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/Cargo.toml
  - README.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/http.rs
  - docs/env.md
-->

---
### Requirement: Token pairing and persistence

`FLEETY_PAIRING_CODE` SHALL, when passed once, enroll a new device: the server mints a token in the `Welcome` message and the daemon writes it to `~/.fleety/fleetyd.token`. On later starts the daemon SHALL load that persisted token. `FLEETY_TOKEN` SHALL override the persisted token when set.

#### Scenario: pairing persists a minted token

- **WHEN** the daemon starts with `FLEETY_PAIRING_CODE` set and no stored token
- **THEN** it receives a minted token in `Welcome` and writes it to `~/.fleety/fleetyd.token` for reuse

<!-- @trace
source: baseline-config-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-discuss.md
  - .spectra.yaml
  - CLAUDE.md
  - .opencode/commands/spectra-ask.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-debug/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
-->