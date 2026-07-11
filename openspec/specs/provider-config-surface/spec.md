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

The `config` command surface SHALL manage the two-tier model: `config provider add <name> --type api --base-url <url> [--key <secret>]` and `config provider add <name> --type oauth:codex`, plus `provider set`, `provider remove`, and `provider list` (listing SHALL show each provider by `type` with its type-appropriate fields and mask secrets). Model roles SHALL be managed with `config model set <main|cheap> --member <provider>/<model> [--stream] [--modalities <list>] [--effort <level>] [--member …] --strategy <single|round_robin|failover>`, plus `model show` and `model unset`. Removing a provider that a role member references SHALL be refused.

#### Scenario: add a provider then bind a model role to it

- **WHEN** `config provider add openai1 --type api --base-url https://api.openai.com/v1 --key sk-x` then `config model set main --member openai1/gpt-4o --strategy single` run
- **THEN** `providers.toml` holds provider `openai1` (type api) and a `main` role with one member `openai1/gpt-4o`

#### Scenario: an oauth provider takes no base_url or key on the command line

- **WHEN** `config provider add codex1 --type oauth:codex` runs
- **THEN** it is accepted with no `base_url`/`key`, and the token is obtained separately via `fleety auth login codex1`


<!-- @trace
source: provider-model-two-tier
updated: 2026-07-10
code:
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: An interactive screen manages providers on a TTY

When stdout is a TTY, `config provider edit` SHALL open an interactive screen listing providers, groups, and roles, supporting add/edit/remove of a provider, setting a group's members and strategy, and binding a role. By default the screen SHALL edit the **connected server's** provider configuration: it is loaded from a structured config snapshot, edited in memory, and written back through a structured config apply under the snapshot's optimistic-lock revision — validation and the atomic write run on the server, with the same semantics as the local path. With an explicit `--target local`, the screen SHALL edit this host's own providers file exactly as before. Against a server that does not advertise credential-era protocol support (config protocol < 2), the remote screen SHALL refuse up front with an update-the-server error before opening — an older server would silently ignore the write-back. A validation failure SHALL be shown without writing; a concurrent-edit conflict SHALL be reported and the screen reloaded from a fresh snapshot rather than overwriting. Provider keys SHALL be masked in the display. When stdout is not a TTY, the system SHALL fall back to the subcommands.

#### Scenario: editing on a TTY saves through validation

- **WHEN** a provider is added in the interactive screen and saved
- **THEN** the configuration is validated and written atomically on the target host, and the key is masked on screen

#### Scenario: default target edits the connected server

- **WHEN** `config provider edit` runs on a TTY with no explicit target while connected to a remote server
- **THEN** the screen shows the server's providers, and saving updates the server's providers file — nothing is written on the CLI host

#### Scenario: explicit local target keeps the local file path

- **WHEN** `config --target local provider edit` runs on a TTY
- **THEN** the screen edits this host's own providers file with unchanged behavior

#### Scenario: old server is refused before the screen opens

- **WHEN** the remote screen is requested against a server advertising config protocol below 2
- **THEN** the command fails up front telling the user to update the server, and no editor opens

#### Scenario: concurrent edit surfaces as a conflict

- **WHEN** the server's configuration changed while the screen was open and the user saves
- **THEN** the save is rejected as a conflict and the screen reloads the current server state instead of overwriting

#### Scenario: non-TTY falls back

- **WHEN** `config provider edit` is invoked without a TTY
- **THEN** the interactive screen does not open and the subcommand path is used


<!-- @trace
source: provider-edit-remote
updated: 2026-07-11
code:
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
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