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

---
### Requirement: Interactive screen edits an existing provider in place

The interactive `config provider edit` screen SHALL let the operator edit the fields of an already-defined provider (base URL, model, key) without removing and re-adding it, reusing the same field-update semantics as the `config provider set` subcommand so that only the fields the operator changes are updated and unchanged fields are preserved. A provider that a group or role references SHALL be editable without first unbinding it. The edited configuration SHALL be validated and written through the shared atomic writer on save.

#### Scenario: editing a referenced provider's model keeps its bindings

- **WHEN** a provider that a group or role references is selected, its model is changed in the interactive screen, and the screen is saved
- **THEN** the provider retains its group and role bindings, only the model field changes, and the file is written through validation

#### Scenario: unchanged fields are preserved on edit

- **WHEN** a provider's key is edited while its base URL and model are left untouched
- **THEN** the saved provider keeps its original base URL and model and only the key changes


<!-- @trace
source: provider-editor-usability
updated: 2026-07-10
code:
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-daemon/src/main.rs
  - README.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install.sh
  - crates/fleety-server/src/schedules.rs
  - docs/env.md
  - Dockerfile
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Interactive screen removes groups and unsets roles

The interactive `config provider edit` screen SHALL support removing a group and unsetting a role, matching the `config group remove` and `config role unset` subcommands. Removing a group that a role still targets SHALL be rejected with a message naming the referring role and SHALL leave the group in place. Unsetting a role that is not defined SHALL be reported without changing the configuration, and unsetting a defined role SHALL remove its binding.

#### Scenario: removing a referenced group is blocked

- **WHEN** a group removal is requested in the interactive screen while a role still targets that group
- **THEN** the removal is rejected with a message naming the referring role and the group remains

#### Scenario: unsetting a role removes its binding

- **WHEN** a defined role is unset in the interactive screen and the screen is saved
- **THEN** the role no longer appears and the configuration is written through validation


<!-- @trace
source: provider-editor-usability
updated: 2026-07-10
code:
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-daemon/src/main.rs
  - README.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install.sh
  - crates/fleety-server/src/schedules.rs
  - docs/env.md
  - Dockerfile
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Interactive screen validates provider fields per field

The interactive `config provider edit` screen SHALL collect a new or edited provider's fields through per-field prompts rather than a single comma-separated line. An empty required field (name, base URL, or model) SHALL be rejected with a message identifying the offending field, and the provider SHALL NOT be added or updated until every required field is non-empty.

#### Scenario: an empty required field is rejected by name

- **WHEN** the per-field provider entry is completed with an empty model field
- **THEN** the entry is rejected with a message naming the model field and no provider is added or updated

#### Scenario: a completed per-field entry adds the provider

- **WHEN** name, base URL, and model are all supplied through the per-field prompts and the entry is confirmed
- **THEN** the provider is added with those field values


<!-- @trace
source: provider-editor-usability
updated: 2026-07-10
code:
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-daemon/src/main.rs
  - README.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install.sh
  - crates/fleety-server/src/schedules.rs
  - docs/env.md
  - Dockerfile
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Interactive screen confirms provider deletion

The interactive `config provider edit` screen SHALL require an explicit confirmation before deleting a provider, so that a single keypress cannot remove a provider. The confirmation prompt SHALL name the provider being deleted. Accepting the confirmation SHALL remove the provider (subject to the existing group/role reference guard); cancelling SHALL leave the configuration unchanged.

#### Scenario: delete asks for confirmation

- **WHEN** the delete key is pressed on a selected provider
- **THEN** the screen enters a confirmation prompt naming the provider and no provider is removed until the confirmation is accepted

#### Scenario: cancelling delete keeps the provider

- **WHEN** the delete confirmation is cancelled
- **THEN** the selected provider remains in the configuration unchanged

<!-- @trace
source: provider-editor-usability
updated: 2026-07-10
code:
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-daemon/src/main.rs
  - README.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install.sh
  - crates/fleety-server/src/schedules.rs
  - docs/env.md
  - Dockerfile
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->