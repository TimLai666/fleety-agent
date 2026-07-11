## MODIFIED Requirements

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
