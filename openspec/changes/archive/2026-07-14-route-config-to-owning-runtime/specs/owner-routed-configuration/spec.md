## ADDED Requirements

### Requirement: CLI configuration routes by owning runtime

The CLI SHALL determine the owner of a flat config key from the registry before performing I/O. Server scope SHALL route to the connected fleety-server. Daemon and Shared scopes SHALL route to fleetyd for the selected device. Cli scope SHALL be handled only by the fleety CLI. Provider, model, and credential operations SHALL route to the connected server.

#### Scenario: server key routes to the server

- **WHEN** the user runs fleety config set FLEETY_ADDR 127.0.0.1:8787 without an explicit target
- **THEN** the CLI sends the mutation to the connected server and does not modify config.toml on the CLI host

#### Scenario: daemon key routes to fleetyd

- **WHEN** the user runs fleety config set FLEETY_PRESENCE on without an explicit target
- **THEN** the CLI sends the mutation to fleetyd for the current device id and neither the CLI nor server writes the daemon config file

#### Scenario: shared key routes to fleetyd

- **WHEN** the user runs fleety config set FLEETY_TZ Asia/Taipei without an explicit target
- **THEN** the CLI sends the Shared-scope mutation to fleetyd and does not write it through the Cli owner path

#### Scenario: cli key stays with the CLI owner

- **WHEN** the user runs fleety config set FLEETY_VOICE_AUDIO auto without an explicit target
- **THEN** only the Cli-scoped value is persisted by the CLI owner

### Requirement: Explicit targets enforce ownership

The CLI SHALL accept server, daemon, cli, and a device id as explicit config targets. local SHALL remain an alias for cli for command compatibility. A target that does not own the requested key SHALL fail before mutation and SHALL identify the correct owner.

#### Scenario: foreign server key is rejected by daemon target

- **WHEN** the user runs fleety config --target daemon set FLEETY_ADDR 0.0.0.0:8787
- **THEN** the command fails before sending or writing and identifies server as the owner

#### Scenario: foreign daemon key is rejected by server target

- **WHEN** the user runs fleety config --target server set FLEETY_DEVICE_ID laptop
- **THEN** the command fails and neither runtime persists the value

#### Scenario: local alias is cli only

- **WHEN** the user runs fleety config --target local set FLEETY_TZ Asia/Taipei
- **THEN** the command fails because Shared is daemon-owned and identifies daemon as the owner

### Requirement: Routing failures never fall back to config files

If the target server or fleetyd is unavailable, rejects the request, times out, or returns a malformed reply, the CLI SHALL fail with a non-zero exit status. It SHALL NOT modify config.toml or providers.toml as a fallback. Connection-profile healing remains governed by the connection-profiles specification and is not a config persistence fallback.

#### Scenario: unavailable owner does not trigger a local write

- **GIVEN** the selected owner is unreachable and local config file bytes are recorded
- **WHEN** the user runs a config mutation owned by that runtime
- **THEN** the command exits non-zero and every recorded local config file remains byte-for-byte unchanged

### Requirement: Config mutation rejects corrupt input and no-op identity keys

Every config mutation SHALL strict-load a present file before modifying it. A malformed file SHALL be reported and left byte-for-byte unchanged. FLEETY_DEVICE_ID SHALL NOT be accepted as a config key because the stable identity is owned by connections.toml.

#### Scenario: corrupt config is preserved

- **GIVEN** config.toml contains malformed TOML and its bytes are recorded
- **WHEN** any owner processes a set or unset command
- **THEN** the command fails and the recorded bytes are unchanged

#### Scenario: device id config key is rejected

- **WHEN** the user runs fleety config set FLEETY_DEVICE_ID laptop
- **THEN** the command reports that device identity is managed through connections.toml and writes nothing

#### Scenario: daemon offline causes no write

- **GIVEN** the target fleetyd is not connected and the local config file bytes are recorded
- **WHEN** a daemon-owned set command is run
- **THEN** the command exits non-zero and the config file bytes remain unchanged

#### Scenario: server rejection causes no local write

- **GIVEN** the server replies with ConfigResult ok false
- **WHEN** a server-owned set command is run
- **THEN** the CLI exits non-zero and does not modify local config.toml or providers.toml

### Requirement: Usage and command failures are machine-detectable

Unknown commands, missing required arguments, malformed target flags, ConfigResult ok false, and ServerMsg Error SHALL produce a non-zero exit status. Bare help requests and explicit help flags SHALL produce a zero exit status.

#### Scenario: remote config rejection is non-zero

- **WHEN** the server rejects a config mutation with ConfigResult ok false
- **THEN** the CLI prints the actionable error and exits non-zero

#### Scenario: unknown command is non-zero

- **WHEN** the user runs an unknown top-level or group subcommand
- **THEN** the CLI prints usage with the unknown command and exits non-zero

#### Scenario: explicit help succeeds

- **WHEN** the user runs fleety --help or a group help command
- **THEN** usage is printed and the process exits zero
