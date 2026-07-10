## ADDED Requirements

### Requirement: The server exposes its settings as a structured snapshot

The protocol SHALL provide a `ConfigSnapshot` request and a `ConfigSnapshotResult` reply that carries the server's settings as structured entries — each with its key, scope, current value, default, description, whether it is a secret, whether it is explicitly set, when a change takes effect, and any enumerated choices — plus the structured provider/model configuration. This replaces the string-in/text-out `ConfigExec` for reads when both ends support it, so a client can render and edit settings structurally rather than parsing rendered text.

#### Scenario: a snapshot carries structured entries

- **WHEN** a client sends `ConfigSnapshot { target: server }` to a supporting server
- **THEN** it receives a `ConfigSnapshotResult` whose entries include, for each setting, its scope/default/description/effect and (for enums) its choices, plus the structured provider/model config

### Requirement: Config changes apply atomically under optimistic locking

A `ConfigApply` SHALL carry a `base_revision` (the snapshot's revision) and a sparse list of changes. The server SHALL reject the apply with a conflict when `base_revision` no longer matches the current config revision (a concurrent edit happened), rather than silently overwriting — preventing lost updates. When the revision matches, the changes SHALL be applied and validated as a set.

#### Scenario: a stale apply is rejected as a conflict

- **GIVEN** a client holds a snapshot at revision R
- **AND** the server's config has since changed (revision is now R')
- **WHEN** the client sends `ConfigApply { base_revision: R, … }`
- **THEN** the server returns a conflict result and applies nothing

### Requirement: Secrets are snapshot as is-set and applied write-only

A `ConfigSnapshotResult` SHALL report a secret setting only as whether it is set (never its value). A `ConfigApply` change to a secret SHALL be tri-state — keep (no change), set (a real new value), or clear — and a masked placeholder SHALL never be written back as if it were a value.

#### Scenario: a secret's value is never echoed or round-tripped

- **WHEN** a snapshot includes a secret setting that is set
- **THEN** the entry reports `is_set = true` and does not include the secret's value
- **AND** a subsequent apply that does not change that secret carries a `keep` (not the masked value)

### Requirement: The protocol negotiates capability and tolerates unknown frames

`Welcome` SHALL carry an additive config-protocol version so a client can choose the structured `ConfigSnapshot`/`ConfigApply` path or fall back to the legacy `ConfigExec`. An unknown inbound frame SHALL NOT drop the connection — the receiver SHALL reply with an `unsupported` error frame and stay connected — so future additive frames never break a live link. `PROTOCOL_VERSION` is incremented.

#### Scenario: an old server makes the client fall back

- **WHEN** a new client connects to a server whose Welcome reports no structured-config support
- **THEN** the client uses the legacy `ConfigExec` path instead of `ConfigSnapshot`

#### Scenario: an unknown frame does not disconnect

- **WHEN** a server receives a frame type it does not recognize
- **THEN** it replies with an `unsupported` error and the connection stays open

### Requirement: Server config is saved atomically and never fail-softs to defaults

The server's `config.toml` write SHALL be atomic (temp file + rename) under a single per-file lock, and a present-but-broken config file SHALL be a clear error rather than a silent fail-soft revert to defaults.

#### Scenario: a broken config file errors instead of reverting

- **WHEN** the server reads a `config.toml` that is present but unparseable during an apply
- **THEN** it returns a clear error rather than silently applying defaults
