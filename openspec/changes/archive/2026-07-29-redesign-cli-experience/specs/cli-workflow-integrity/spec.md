## ADDED Requirements

### Requirement: Every command node has exhaustive parser coverage

The command parser SHALL be defined independently from command execution and SHALL be exhaustively tested for help aliases, missing values, trailing arguments, global-option placement, compatibility aliases, and exit classes at every command node.

#### Scenario: generated command inventory is fully tested

- **WHEN** a command or subgroup is added to the typed command inventory
- **THEN** a parser test SHALL fail until its three help spellings, invalid trailing input, and canonical invocation are covered

##### Example: newly added command node

- **GIVEN** `fleety completion` is present in generated top-level help
- **WHEN** its inventory row or `help`, `--help`, `-h`, invalid-extra-argument, or valid-shell case is removed from the exhaustive matrix
- **THEN** the parser coverage test fails before command execution

### Requirement: Compatibility aliases cannot drift from canonical commands

Legacy aliases SHALL map to canonical command values before execution. They SHALL NOT maintain separate network, persistence, validation, or rendering logic.

#### Scenario: alias protocol payload matches canonical payload

- **WHEN** smoke tests run a legacy alias and canonical command against a recording fake Server
- **THEN** the captured owner, message variant, and payload SHALL be equal

##### Example: provider alias payload

- **GIVEN** a recording Server and Provider `codex`
- **WHEN** `fleety provider status codex` and `fleety auth status codex` run against it
- **THEN** both capture the Server owner and the same credential-status message fields
