## MODIFIED Requirements

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

## ADDED Requirements

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
