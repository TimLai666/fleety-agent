# interactive-config-panel Specification

## Purpose

TBD - created by archiving change 'remote-config-panel'. Update Purpose after archive.

## Requirements

### Requirement: Bare fleety config opens a three-region interactive panel

On a TTY, `fleety config` with no arguments SHALL open a single interactive panel with three regions — Connection, This device, and Server — switchable without any `--target` flag. The Connection region edits `connections.toml`, the This-device region edits the local Cli/Shared settings, and the Server region edits the connected server's settings and provider/model configuration. Without a TTY, `fleety config` SHALL fall back to the non-interactive text commands.

#### Scenario: the panel exposes all three layers from one entry

- **WHEN** `fleety config` runs on a TTY
- **THEN** a panel opens with Connection / This device / Server regions, and switching regions needs no `--target` flag

#### Scenario: no TTY falls back to text

- **WHEN** `fleety config list` runs without a TTY
- **THEN** it uses the non-interactive text command path, not the panel


<!-- @trace
source: remote-config-panel
updated: 2026-07-10
code:
  - docs/design-cli-config.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - docs/roadmap.md
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: The server region edits remote settings via the structured channel

The Server region SHALL populate from a `ConfigSnapshot` and apply edits via `ConfigApply` when the server supports the structured protocol, falling back to the legacy `ConfigExec` text flow otherwise. Secret fields SHALL be masked and write-only (edits send a new value or clear, never the masked placeholder); a provider's fields SHALL render per its `type`; and when a change takes effect SHALL be shown.

#### Scenario: server settings edit remotely and show effect timing

- **GIVEN** the panel's Server region is populated from a snapshot of a supporting server
- **WHEN** the user changes a setting and applies it
- **THEN** the change is sent as a `ConfigApply` and the result shows when it takes effect (next connection or restart)


<!-- @trace
source: remote-config-panel
updated: 2026-07-10
code:
  - docs/design-cli-config.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - docs/roadmap.md
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Sensitive server-key changes require auth and are warned and audited

A `ConfigApply` that mutates a Server-scope setting SHALL require the server to have authentication enabled (per the auth-default-on rule). Overwriting a key that could redirect data or credentials off-box (a provider `base_url`/`key`, the backup repo/token, an oauth endpoint) SHALL prompt a prominent confirmation and be recorded in the audit log (with old/new host), and a secret SHALL be reported in a snapshot only as is-set with the read recorded.

#### Scenario: overwriting an exfiltration-risk key warns and audits

- **WHEN** the user changes a provider's `base_url` (a data-redirect risk) in the panel and applies it
- **THEN** a prominent confirmation is shown before applying, and the change is written to the audit log

<!-- @trace
source: remote-config-panel
updated: 2026-07-10
code:
  - docs/design-cli-config.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - docs/roadmap.md
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: The panel Connection region offers the local server

When the three-region `fleety config` panel opens, it SHALL probe for a local server on loopback with a short timeout and, when one answers and no saved profile already points at it, list a `local` entry in the Connection region (in addition to the saved profiles) using the same discovery the guided init uses. Selecting it with the existing switch/save keys SHALL make `local` the current profile and persist it — no pairing code is required because the local connection is loopback-trusted. When no local server answers, or a profile already points at it, the Connection region SHALL behave exactly as before (saved profiles only). The in-memory `local` entry SHALL NOT be written to disk unless the user saves.

#### Scenario: local server appears and is selectable

- **WHEN** the panel opens on a host whose local server answers and no profile points at it
- **THEN** a `local` entry appears in the Connection region, and switching to it and saving persists a `local` profile made current, without a pairing code

#### Scenario: no local server leaves the region unchanged

- **WHEN** the panel opens on a host with no local server, or a profile already targets the local URL
- **THEN** the Connection region lists only the saved profiles, as before

#### Scenario: an unsaved local entry is not persisted

- **WHEN** the panel shows the injected `local` entry but the user does not save
- **THEN** no `local` profile is written to connections.toml

<!-- @trace
source: connection-surface-consistency
updated: 2026-07-12
code:
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/config_panel.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->