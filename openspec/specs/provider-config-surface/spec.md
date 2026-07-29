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

When stdout is a TTY, config provider edit SHALL open an interactive screen listing providers, groups, and roles, supporting add, edit, and remove of a provider, setting a group's members and strategy, and binding a role. The screen SHALL always edit the connected server's provider configuration: it is loaded from a structured config snapshot, edited in memory, and written back through structured config apply under the snapshot revision. Validation and atomic persistence SHALL run on the server. An explicit cli or local target SHALL be rejected before the editor opens. Against a server below config protocol 2, the screen SHALL fail before opening with an update instruction. A validation failure SHALL be shown without writing. A concurrent-edit conflict SHALL be reported and the screen SHALL reload from a fresh snapshot rather than overwrite. Provider keys SHALL be masked. Without a TTY, the system SHALL use provider subcommands, which also target the server.

#### Scenario: editing on a TTY saves through server validation

- **WHEN** a provider is added in the interactive screen and saved
- **THEN** the configuration is validated and written atomically by the connected server and the key is masked on screen

#### Scenario: default target edits the connected server

- **WHEN** config provider edit runs on a TTY with no explicit target
- **THEN** the screen shows the server providers, saving updates the server providers file, and nothing is written on the CLI host

#### Scenario: explicit local target is rejected

- **WHEN** config --target local provider edit runs on a TTY
- **THEN** the command fails before the editor opens and directs the user to the connected server flow

#### Scenario: old server is refused before the screen opens

- **WHEN** the remote screen is requested against a server advertising config protocol below 2
- **THEN** the command fails with an update instruction and no editor opens

#### Scenario: concurrent edit surfaces as a conflict

- **WHEN** the server configuration changes while the screen is open and the user saves
- **THEN** the save is rejected as a conflict and the screen reloads current server state instead of overwriting

#### Scenario: non-TTY uses server subcommands

- **WHEN** config provider edit is invoked without a TTY
- **THEN** the interactive screen does not open and the server-targeted subcommand path is used


<!-- @trace
source: route-config-to-owning-runtime
updated: 2026-07-14
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/provider_tui.rs
  - docs/design-cli-config.md
  - docs/roadmap.md
  - README.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-cli/src/main.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-cli/src/server.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
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

---
### Requirement: Provider, authentication, model catalog, and roles form one workflow

The canonical Provider surface SHALL show each Provider's type, endpoint class, non-secret API-key state, authentication state, catalog state, and bound roles. API Providers SHALL render `key=Set` or `key=Not set` from strictly parsed snapshot metadata; `Set` means a non-empty key and blank keys SHALL be rejected. Malformed metadata SHALL fail the snapshot instead of being silently discarded. JSON Provider lists SHALL retain the existing `data.output` compatibility field and additionally expose each API Provider's state as boolean `data.providers[].key_present` without a secret value. OAuth login, logout, and status SHALL be actions on a named Provider; model selection SHALL proceed through Provider selection, catalog load, model selection or manual ID, and role confirmation.

#### Scenario: OAuth provider status is visible before model selection

- **WHEN** an OAuth Provider is not signed in
- **THEN** the Provider surface SHALL show Not signed in, offer Login, and prevent a catalog request from being represented as an anonymous endpoint failure

##### Example: Codex OAuth before catalog

- **GIVEN** Provider `tingzhen-codex` has type `oauth:codex` and no stored credential
- **WHEN** the user starts main-model selection
- **THEN** the row shows Not signed in and Login, and no model-catalog request is sent until authentication succeeds

#### Scenario: API key presence is visible without exposing the secret

- **GIVEN** the Server snapshot reports key presence for API Provider `openai`
- **WHEN** the user views `fleety provider list` or the Provider TUI
- **THEN** human and TUI rows SHALL show `key=Set`, JSON SHALL report `"key_present": true` for `openai`, and no surface SHALL contain the key value


<!-- @trace
source: redesign-cli-experience
updated: 2026-07-29
code:
  - crates/fleety-tools/src/secure.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-cli/src/commands.rs
  - crates/fleety-textarea/README.md
  - scripts/check-spectra-archive-instructions.sh
  - crates/fleety-cli/src/tui.rs
  - docs/HANDOFF.md
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install-server.sh
  - crates/fleety-markdown/src/latex/math_box.rs
  - docs/STATUS.md
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-inline/src/common.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-textarea/LICENSE
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-markdown/src/parse.rs
  - crates/fleety-cli/src/workspace.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/oauth.rs
  - Cargo.toml
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - AGENTS.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-tools/src/provider_service.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-tools/src/deps/runtime.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-server/src/mdns.rs
  - .opencode/commands/spectra-archive.md
  - crates/fleety-cli/src/server.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-tools/src/device.rs
  - crates/fleety-markdown/src/colors.rs
  - docs/env.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/source_map.rs
  - crates/fleety-markdown/Cargo.toml
  - docs/roadmap.md
  - crates/fleety-cli/src/provider_service.rs
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-textarea/src/lib.rs
  - docs/tools.md
  - crates/fleety-tools/Cargo.toml
  - docs/acp.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - README.md
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-tools/src/chrome.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/input.rs
  - .github/workflows/ci.yml
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/LICENSE
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-inline/README.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-daemon/src/winsvc.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Model discovery failure has retry and manual recovery

Catalog loading SHALL expose Loading, Available, Failed, and Unavailable states. A failure SHALL retain the backend error details and offer Retry and Enter model ID without losing the selected Provider or role.

#### Scenario: retry preserves selection

- **WHEN** catalog loading fails for Provider `tingzhen-codex`, role `main`, and the user selects Retry
- **THEN** the next request SHALL use the same connected Server, Provider, and role, while the previous error remains inspectable until the retry completes


<!-- @trace
source: redesign-cli-experience
updated: 2026-07-29
code:
  - crates/fleety-tools/src/secure.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-cli/src/commands.rs
  - crates/fleety-textarea/README.md
  - scripts/check-spectra-archive-instructions.sh
  - crates/fleety-cli/src/tui.rs
  - docs/HANDOFF.md
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install-server.sh
  - crates/fleety-markdown/src/latex/math_box.rs
  - docs/STATUS.md
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-inline/src/common.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-textarea/LICENSE
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-markdown/src/parse.rs
  - crates/fleety-cli/src/workspace.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/oauth.rs
  - Cargo.toml
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - AGENTS.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-tools/src/provider_service.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-tools/src/deps/runtime.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-server/src/mdns.rs
  - .opencode/commands/spectra-archive.md
  - crates/fleety-cli/src/server.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-tools/src/device.rs
  - crates/fleety-markdown/src/colors.rs
  - docs/env.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/source_map.rs
  - crates/fleety-markdown/Cargo.toml
  - docs/roadmap.md
  - crates/fleety-cli/src/provider_service.rs
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-textarea/src/lib.rs
  - docs/tools.md
  - crates/fleety-tools/Cargo.toml
  - docs/acp.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - README.md
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-tools/src/chrome.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/input.rs
  - .github/workflows/ci.yml
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/LICENSE
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-inline/README.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-daemon/src/winsvc.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Provider commands and TUI share one application service

Canonical provider/model commands, compatibility aliases, and the TUI SHALL use the same validation, owner routing, OAuth status, catalog fetch, role binding, and error mapping service.

#### Scenario: invalid provider input agrees across surfaces

- **WHEN** the same invalid Provider name or endpoint is submitted through command mode and the TUI
- **THEN** both surfaces SHALL reject it before mutation with the same error kind and remediation

##### Example: unsafe Provider name

- **GIVEN** the Provider name is `../outside`
- **WHEN** it is submitted through `fleety provider add` and the TUI add wizard
- **THEN** both return the same validation kind and safe-name remediation, and the Server records no mutation

<!-- @trace
source: redesign-cli-experience
updated: 2026-07-29
code:
  - crates/fleety-tools/src/secure.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-cli/src/commands.rs
  - crates/fleety-textarea/README.md
  - scripts/check-spectra-archive-instructions.sh
  - crates/fleety-cli/src/tui.rs
  - docs/HANDOFF.md
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install-server.sh
  - crates/fleety-markdown/src/latex/math_box.rs
  - docs/STATUS.md
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-inline/src/common.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-textarea/LICENSE
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-markdown/src/parse.rs
  - crates/fleety-cli/src/workspace.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/oauth.rs
  - Cargo.toml
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - AGENTS.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-tools/src/provider_service.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-tools/src/deps/runtime.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-server/src/mdns.rs
  - .opencode/commands/spectra-archive.md
  - crates/fleety-cli/src/server.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-tools/src/device.rs
  - crates/fleety-markdown/src/colors.rs
  - docs/env.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/source_map.rs
  - crates/fleety-markdown/Cargo.toml
  - docs/roadmap.md
  - crates/fleety-cli/src/provider_service.rs
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-textarea/src/lib.rs
  - docs/tools.md
  - crates/fleety-tools/Cargo.toml
  - docs/acp.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - README.md
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-tools/src/chrome.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/input.rs
  - .github/workflows/ci.yml
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/LICENSE
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-inline/README.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-daemon/src/winsvc.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->