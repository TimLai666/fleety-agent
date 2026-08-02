## ADDED Requirements

### Requirement: Durable connection stores declare a compatible writer contract

A present `connections.toml` SHALL declare a supported store format version and a current-writer marker. The shared connection layer SHALL validate both fields before returning a durable store to any caller. A missing, malformed, or unsupported marker SHALL classify the store as incompatible legacy or future state rather than as an empty store. Every current durable writer SHALL emit the supported fields through the shared atomic `0600` write path.

#### Scenario: a current store round-trips its compatibility marker

- **WHEN** a current Fleety binary writes a profile mutation and reloads `connections.toml`
- **THEN** the store format version and writer marker SHALL remain present and the profile SHALL resolve normally

#### Scenario: an old writer rewrite is rejected

- **GIVEN** a current store has established the compatibility contract
- **WHEN** an older binary rewrites the file and removes the current-only marker or fields
- **THEN** every current durable reader SHALL return an incompatible-store error before credential use, network I/O, or profile mutation

#### Scenario: an unmarked legacy store is not silently migrated

- **WHEN** a current binary loads a present `connections.toml` without the supported format version or writer marker
- **THEN** it SHALL preserve the file, refuse durable profile use, and report explicit update-all-binaries and re-pair recovery guidance

#### Scenario: an unsupported future store is preserved

- **WHEN** a current binary loads a store format version newer than it supports
- **THEN** it SHALL refuse to parse the store as an operational profile set and SHALL not rewrite or delete the file

##### Example:

- **GIVEN** `format_version = 99` and a profile containing a token
- **WHEN** `fleety server show` loads the store
- **THEN** the command SHALL return an incompatible-store error and the file SHALL remain byte-for-byte unchanged

### Requirement: Incompatible connection stores have an explicit credential-safe recovery path

A current Fleety surface that encounters an incompatible durable store SHALL expose one consistent recovery instruction. The recovery path SHALL require updated Fleety binaries and explicit user-supplied connection details or a new pairing flow. It SHALL not send a token loaded from the rejected store, guess a learned endpoint, or restore a missing secure-channel proof.

#### Scenario: recovery rebuilds a profile from explicit details

- **GIVEN** the durable store is rejected as incompatible
- **WHEN** the user supplies a profile name, current Server URL, and pairing code through the documented recovery path
- **THEN** Fleety SHALL create a marked store atomically and SHALL establish the profile through the existing authenticated pairing flow

#### Scenario: rejected credentials never reach transport

- **GIVEN** an incompatible store contains a token
- **WHEN** the CLI, TUI, ACP, Doctor, or Daemon attempts to resolve its operational target
- **THEN** the operation SHALL fail before loading that token into a transport or sending it to a Server

#### Scenario: transient targets remain side-effect free

- **WHEN** a caller uses a raw URL, environment URL, ACP target, or Doctor target without selecting a durable profile
- **THEN** it SHALL not create, update, or repair the durable store compatibility marker

##### Example:

- **GIVEN** `connections.toml` is absent and `FLEETY_AGENT_URL=ws://127.0.0.1:8787` is set
- **WHEN** `fleety --url ws://127.0.0.1:8787 status` runs
- **THEN** the command SHALL use the transient target and `connections.toml` SHALL remain absent
