# provider-config-surface Specification

## Purpose

TBD - created by archiving change 'provider-config-surface'. Update Purpose after archive.

## Requirements

### Requirement: providers.toml is written back atomically and validated

The system SHALL be able to serialize the named-provider configuration back to `providers.toml` and write it atomically (write to a temporary file, then rename), so a crash or concurrent write never leaves a half-written file. A parse → modify → write → parse cycle SHALL yield an equivalent model. Before any write, the configuration SHALL be validated; an invalid configuration SHALL NOT be written and SHALL return an error message rather than crashing.

#### Scenario: write then re-read is stable

- **WHEN** a configuration with providers, a group, and role mappings is written and then parsed again
- **THEN** the parsed model equals the written model

#### Scenario: invalid configuration is not written

- **WHEN** a write is attempted with a configuration that fails validation
- **THEN** no file is written and an error is returned


<!-- @trace
source: provider-config-surface
updated: 2026-06-29
code:
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/providers.rs
  - docs/env.md
  - crates/fleety-tools/Cargo.toml
-->

---
### Requirement: Validation rejects inconsistent provider configuration

Validation SHALL be a pure function that rejects: a duplicate provider name; a group whose members include an undefined provider; a group strategy other than `round_robin` or `failover`; and a role mapped to a name that is neither a defined provider nor a defined group. Each rejection SHALL return a message that identifies the offending item. A consistent configuration SHALL pass.

#### Scenario: dangling references are rejected

- **WHEN** a group lists a member that is not a defined provider, or a role targets an undefined name
- **THEN** validation returns an error naming the offending member or target

#### Scenario: a consistent configuration passes

- **WHEN** every group member and role target resolves and names are unique and strategies are valid
- **THEN** validation succeeds


<!-- @trace
source: provider-config-surface
updated: 2026-06-29
code:
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/providers.rs
  - docs/env.md
  - crates/fleety-tools/Cargo.toml
-->

---
### Requirement: config subcommands manage providers, groups, and roles

All three binaries (`fleety`, `fleety-server`, `fleetyd`) SHALL expose `config` subcommands to manage `providers.toml`: `provider add|set|remove|list`, `group set|remove|list`, and `role set|unset|list`. Each mutating subcommand SHALL load the current configuration, apply the change, validate, then write atomically. Listing a provider SHALL mask its key. Removing a provider that is still referenced by a group or role SHALL be rejected with a message naming the referrer. Subcommand parsing SHALL be a pure function; an unknown flag or verb SHALL return an error.

#### Scenario: add then list a provider

- **WHEN** `config provider add foo` is run with a base URL, model, and key, then `config provider list` is run
- **THEN** `foo` appears in the list with its key masked

#### Scenario: removing a referenced provider is blocked

- **WHEN** `config provider remove foo` is run while a group or role still references `foo`
- **THEN** the command fails with a message naming the referrer and the file is unchanged

#### Scenario: adding a duplicate provider name fails

- **WHEN** `config provider add foo` is run and `foo` already exists
- **THEN** the command fails and the file is unchanged


<!-- @trace
source: provider-config-surface
updated: 2026-06-29
code:
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/providers.rs
  - docs/env.md
  - crates/fleety-tools/Cargo.toml
-->

---
### Requirement: An interactive screen manages providers on a TTY

When stdout is a TTY, `config provider edit` SHALL open an interactive screen listing providers, groups, and roles, supporting add/edit/remove of a provider, setting a group's members and strategy, and binding a role. Saving SHALL run the same validation and atomic write as the subcommands; a validation failure SHALL be shown without writing. Provider keys SHALL be masked in the display. When stdout is not a TTY, the system SHALL fall back to the subcommands.

#### Scenario: editing on a TTY saves through validation

- **WHEN** a provider is added in the interactive screen and saved
- **THEN** the configuration is validated and written atomically, and the key is masked on screen

#### Scenario: non-TTY falls back

- **WHEN** `config provider edit` is invoked without a TTY
- **THEN** the interactive screen does not open and the subcommand path is used

<!-- @trace
source: provider-config-surface
updated: 2026-06-29
code:
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/providers.rs
  - docs/env.md
  - crates/fleety-tools/Cargo.toml
-->