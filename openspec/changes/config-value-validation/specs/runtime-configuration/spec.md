## ADDED Requirements

### Requirement: Config write value validation

When a `config set` or interactive edit assigns a value to a known setting that carries a validator in the registry, the system SHALL validate the value before writing it to `config.toml` and SHALL reject any value the validator does not accept, leaving the stored configuration unchanged. This validation SHALL apply identically across every write surface: the shared `config set` dispatch used by `fleety`, `fleety-server`, and `fleetyd` (including remote `--target`), the non-TTY line-based editor, and the CLI ratatui edit screen. A setting that has no validator SHALL accept any value (pass-through), and an empty value SHALL continue to mean unset rather than being validated.

#### Scenario: invalid boolean is rejected

- **WHEN** a user runs `config set FLEETY_REQUIRE_AUTH abc` (the setting accepts only `0` or `1`)
- **THEN** the command SHALL fail without modifying `config.toml`

#### Scenario: invalid enum is rejected

- **WHEN** a user runs `config set FLEETY_FS_SCOPE ful` (the setting accepts only `full` or `workspace`)
- **THEN** the command SHALL fail without modifying `config.toml`

#### Scenario: out-of-domain numeric is rejected

- **WHEN** a user runs `config set FLEETY_CMD_TIMEOUT_SECS notanumber` (the setting accepts a non-negative integer)
- **THEN** the command SHALL fail without modifying `config.toml`

#### Scenario: valid value persists

- **WHEN** a user runs `config set FLEETY_POLICY require_approval` (an accepted enum member)
- **THEN** the value SHALL be written to `config.toml` under the setting's scope

#### Scenario: interactive edit rejects invalid value without saving

- **WHEN** the ratatui or line-based editor commits an invalid value for a validated setting
- **THEN** the editor SHALL NOT save the change and SHALL surface the validation error to the user

#### Scenario: unvalidated key passes through

- **WHEN** a user runs `config set FLEETY_TZ Anything/Here` (a setting with no validator)
- **THEN** the value SHALL be accepted and written unchanged

### Requirement: Validation error names accepted values

When a config write is rejected by a setting's validator, the returned error message SHALL name the key and describe the accepted values (the enum members, the boolean form `0|1`, the numeric domain, or the required URL scheme), so the user can correct the value without inspecting source code.

#### Scenario: enum error lists members

- **WHEN** `config set FLEETY_VOICE_AUDIO loud` is rejected (accepted: `auto`, `on`, `off`)
- **THEN** the error message SHALL name `FLEETY_VOICE_AUDIO` and list the accepted values `auto`, `on`, `off`

#### Scenario: URL error states required scheme

- **WHEN** `config set FLEETY_MODEL_BASE_URL notaurl` is rejected (requires an `http://` or `https://` URL)
- **THEN** the error message SHALL name the key and state that an `http`/`https` URL is required
