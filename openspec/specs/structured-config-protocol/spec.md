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

A ConfigApply SHALL carry a base_revision and a sparse list of changes, and SHALL accept an optional full structured provider configuration in providers_json. The owner SHALL reject an apply with conflict when base_revision no longer matches the current revision and SHALL apply nothing. The server revision SHALL fingerprint settings and providers. Before any file is written, all key changes and providers_json SHALL be parsed and validated as one transaction. A parse or validation failure SHALL leave both files byte-for-byte unchanged. Accepted provider write-backs SHALL be audited without recording key values.

#### Scenario: a stale apply is rejected as a conflict

- **GIVEN** a client holds a snapshot at revision R
- **AND** the owner configuration has changed to revision R2
- **WHEN** the client sends ConfigApply with base_revision R
- **THEN** the owner returns conflict and applies nothing

#### Scenario: provider write-back lands atomically under the same lock

- **GIVEN** a client holds server snapshot revision R
- **WHEN** it sends a valid ConfigApply with providers_json and R still matches
- **THEN** the server validates and atomically writes the providers file and replies success

#### Scenario: a provider edit invalidates stale snapshots

- **GIVEN** a client holds server snapshot revision R
- **AND** another client applies a provider change
- **WHEN** the first client sends any ConfigApply with revision R
- **THEN** the server returns conflict and applies nothing

#### Scenario: malformed provider payload rolls back key changes

- **GIVEN** a ConfigApply contains a valid flat-key change and malformed or invalid providers_json
- **WHEN** the server processes the apply
- **THEN** it returns an actionable error and neither config.toml nor providers.toml is modified


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

---
### Requirement: Device targets are executed by fleetyd

The server SHALL support ConfigExec, ConfigSnapshot, and ConfigApply with ConfigTarget Device by routing a reserved request to the connected fleetyd for that device id and returning its result. fleetyd SHALL restrict these operations to Daemon and Shared scopes. Reserved config operations SHALL NOT be advertised as agent-callable device tools.

#### Scenario: device exec returns daemon-owned output

- **GIVEN** fleetyd for device laptop is connected
- **WHEN** a client sends ConfigExec targeting Device laptop for FLEETY_PRESENCE
- **THEN** fleetyd executes the scoped operation and the server returns its ConfigResult

#### Scenario: device snapshot excludes foreign scopes

- **WHEN** a client requests ConfigSnapshot for a connected device
- **THEN** the entries contain only Daemon and Shared settings and contain no Cli or Server settings

#### Scenario: device apply uses daemon revision

- **GIVEN** a client holds a daemon snapshot revision R
- **WHEN** it sends a valid ConfigApply targeting that device with revision R
- **THEN** fleetyd validates and persists the change and returns success

#### Scenario: disconnected device fails without fallback

- **WHEN** a client sends a device config request for a fleetyd that is not connected
- **THEN** the server returns an actionable not-connected error and does not write that device configuration

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