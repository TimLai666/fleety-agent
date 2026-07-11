# structured-config-protocol Specification

## Purpose

TBD - created by archiving change 'remote-config-panel'. Update Purpose after archive.

## Requirements

### Requirement: The server exposes its settings as a structured snapshot

The protocol SHALL provide a `ConfigSnapshot` request and a `ConfigSnapshotResult` reply that carries the server's settings as structured entries — each with its key, scope, current value, default, description, whether it is a secret, whether it is explicitly set, when a change takes effect, and any enumerated choices — plus the structured provider/model configuration. This replaces the string-in/text-out `ConfigExec` for reads when both ends support it, so a client can render and edit settings structurally rather than parsing rendered text.

#### Scenario: a snapshot carries structured entries

- **WHEN** a client sends `ConfigSnapshot { target: server }` to a supporting server
- **THEN** it receives a `ConfigSnapshotResult` whose entries include, for each setting, its scope/default/description/effect and (for enums) its choices, plus the structured provider/model config


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
### Requirement: Config changes apply atomically under optimistic locking

A `ConfigApply` SHALL carry a `base_revision` (the snapshot's revision) and a sparse list of changes, and MAY additionally carry a full structured provider configuration (`providers_json`, additive and optional; the same shape the snapshot returns). The server SHALL reject the apply with a conflict when `base_revision` no longer matches the current config revision (a concurrent edit happened), rather than silently overwriting — preventing lost updates. The config revision SHALL fingerprint both the settings file and the providers file, so provider edits and key edits each invalidate stale snapshots of the other. When the revision matches, the key changes SHALL be applied and validated as a set; when `providers_json` is present it SHALL be parsed and validated, then written to the server's providers file with the existing atomic write — a parse or validation failure SHALL be rejected without writing anything. Accepted provider write-backs SHALL be audited as a provider-configuration change without recording key values.

#### Scenario: a stale apply is rejected as a conflict

- **GIVEN** a client holds a snapshot at revision R
- **AND** the server's config has since changed (revision is now R')
- **WHEN** the client sends `ConfigApply { base_revision: R, … }`
- **THEN** the server returns a conflict result and applies nothing

#### Scenario: provider write-back lands atomically under the same lock

- **GIVEN** a client holds a snapshot at revision R with the server's providers
- **WHEN** it sends `ConfigApply { base_revision: R, providers_json: <edited config> }` and R still matches
- **THEN** the server validates and atomically writes the providers file and replies success

#### Scenario: a provider edit invalidates stale snapshots

- **GIVEN** a client holds a snapshot at revision R
- **AND** another client has since applied a provider change (revision is now R')
- **WHEN** the first client sends any `ConfigApply { base_revision: R, … }`
- **THEN** the server returns a conflict result and applies nothing

#### Scenario: malformed provider payload is rejected without side effects

- **WHEN** a `ConfigApply` carries a `providers_json` that fails parsing or validation
- **THEN** the server replies with an actionable error and the providers file is not modified


<!-- @trace
source: provider-edit-remote
updated: 2026-07-11
code:
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
-->

---
### Requirement: Secrets are snapshot as is-set and applied write-only

A `ConfigSnapshotResult` SHALL report a secret setting only as whether it is set (never its value). A `ConfigApply` change to a secret SHALL be tri-state — keep (no change), set (a real new value), or clear — and a masked placeholder SHALL never be written back as if it were a value.

#### Scenario: a secret's value is never echoed or round-tripped

- **WHEN** a snapshot includes a secret setting that is set
- **THEN** the entry reports `is_set = true` and does not include the secret's value
- **AND** a subsequent apply that does not change that secret carries a `keep` (not the masked value)


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
### Requirement: The protocol negotiates capability and tolerates unknown frames

`Welcome` SHALL carry an additive config-protocol version so a client can choose the structured `ConfigSnapshot`/`ConfigApply` path or fall back to the legacy `ConfigExec`. An unknown inbound frame SHALL NOT drop the connection — the receiver SHALL reply with an `unsupported` error frame and stay connected — so future additive frames never break a live link. `PROTOCOL_VERSION` is incremented.

#### Scenario: an old server makes the client fall back

- **WHEN** a new client connects to a server whose Welcome reports no structured-config support
- **THEN** the client uses the legacy `ConfigExec` path instead of `ConfigSnapshot`

#### Scenario: an unknown frame does not disconnect

- **WHEN** a server receives a frame type it does not recognize
- **THEN** it replies with an `unsupported` error and the connection stays open


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
### Requirement: Server config is saved atomically and never fail-softs to defaults

The server's `config.toml` write SHALL be atomic (temp file + rename) under a single per-file lock, and a present-but-broken config file SHALL be a clear error rather than a silent fail-soft revert to defaults.

#### Scenario: a broken config file errors instead of reverting

- **WHEN** the server reads a `config.toml` that is present but unparseable during an apply
- **THEN** it returns a clear error rather than silently applying defaults

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