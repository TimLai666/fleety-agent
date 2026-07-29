# interactive-config-panel Specification

## Purpose

TBD - created by archiving change 'remote-config-panel'. Update Purpose after archive.

## Requirements

### Requirement: Bare fleety config opens a four-region interactive panel

On a TTY, fleety config with no arguments SHALL open a single interactive panel with four regions: Connection, CLI, Daemon, and Server. The Connection region manages connection profiles. The CLI region edits only Cli-scoped settings. The Daemon region loads and applies Daemon and Shared settings through fleetyd. The Server region loads and applies Server settings through fleety-server. Without a TTY, fleety config SHALL use the non-interactive text command path.

#### Scenario: the panel exposes all four owners from one entry

- **WHEN** fleety config runs on a TTY
- **THEN** a panel opens with Connection, CLI, Daemon, and Server regions and switching regions needs no target flag

#### Scenario: no TTY uses text commands

- **WHEN** fleety config list runs without a TTY
- **THEN** it uses the non-interactive text command path


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
### Requirement: The server region edits remote settings via the structured channel

The Server region SHALL populate from a `ConfigSnapshot` and apply edits via `ConfigApply` when the server supports the structured protocol, falling back to the legacy `ConfigExec` text flow otherwise. Secret fields SHALL be masked and write-only (edits send a new value or clear, never the masked placeholder); a provider's fields SHALL render per its `type`; and when a change takes effect SHALL be shown.

#### Scenario: server settings edit remotely and show effect timing

- **GIVEN** the panel's Server region is populated from a snapshot of a supporting server
- **WHEN** the user changes a setting and applies it
- **THEN** the change is sent as a `ConfigApply` and the result shows when it takes effect (next connection or restart)


<!-- @trace
source: remote-config-panel
updated: 2026-07-10
code:
  - docs/design-cli-config.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - docs/roadmap.md
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Sensitive server-key changes require auth and are warned and audited

A `ConfigApply` that mutates a Server-scope setting SHALL require the server to have authentication enabled (per the auth-default-on rule). Overwriting a key that could redirect data or credentials off-box (a provider `base_url`/`key`, the backup repo/token, an oauth endpoint) SHALL prompt a prominent confirmation and be recorded in the audit log (with old/new host), and a secret SHALL be reported in a snapshot only as is-set with the read recorded.

#### Scenario: overwriting an exfiltration-risk key warns and audits

- **WHEN** the user changes a provider's `base_url` (a data-redirect risk) in the panel and applies it
- **THEN** a prominent confirmation is shown before applying, and the change is written to the audit log

<!-- @trace
source: remote-config-panel
updated: 2026-07-10
code:
  - docs/design-cli-config.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - docs/roadmap.md
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: The panel Connection region offers the local server

When the three-region `fleety config` panel opens, it SHALL probe for a local server on loopback with a short timeout and, when one answers and no saved profile already points at it, list a `local` entry in the Connection region (in addition to the saved profiles) using the same discovery the guided init uses. Selecting it with the existing switch/save keys SHALL make `local` the current profile and persist it — no pairing code is required because the local connection is loopback-trusted. When no local server answers, or a profile already points at it, the Connection region SHALL behave exactly as before (saved profiles only). The in-memory `local` entry SHALL NOT be written to disk unless the user saves.

#### Scenario: local server appears and is selectable

- **WHEN** the panel opens on a host whose local server answers and no profile points at it
- **THEN** a `local` entry appears in the Connection region, and switching to it and saving persists a `local` profile made current, without a pairing code

#### Scenario: no local server leaves the region unchanged

- **WHEN** the panel opens on a host with no local server, or a profile already targets the local URL
- **THEN** the Connection region lists only the saved profiles, as before

#### Scenario: an unsaved local entry is not persisted

- **WHEN** the panel shows the injected `local` entry but the user does not save
- **THEN** no `local` profile is written to connections.toml

<!-- @trace
source: connection-surface-consistency
updated: 2026-07-12
code:
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/config_panel.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Bare `fleety config` opens a top-level menu with guided drill-down

On a TTY with no subcommand, `fleety config` SHALL open a top-level menu from which the user selects what to configure — at least Providers, Models, and Settings — and selecting an item SHALL enter that item's own screen. `Esc` SHALL return from a screen to the menu; a quit key SHALL exit. When not on a TTY, or when a subcommand is given, the existing non-interactive behavior SHALL be preserved.

#### Scenario: menu drill-down and back

- **WHEN** the user runs `fleety config` on a TTY, selects "Providers", then presses Esc
- **THEN** the Providers screen opens and Esc returns to the top-level menu (without exiting the program)


<!-- @trace
source: interactive-config-menu
updated: 2026-07-12
code:
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-server/src/main.rs
  - docs/design-cli-config.md
  - README.md
-->

---
### Requirement: Guided provider and model editing

Adding a provider SHALL be guided: the user selects the provider type from a menu of the registered types, then is prompted for each required field in turn (name, and for an api type its base_url and api key) rather than entering one delimited line. Setting a model role SHALL be two-level: the user first selects a provider, then selects that provider's model. For an api provider with a base_url, the editor SHALL fetch the provider's model list from its `/models` endpoint and present it as a searchable, selectable list. For an `oauth:codex` provider in the remote Server region, the editor SHALL request model IDs through the server's provider-model discovery operation when the connected server supports it. If discovery fails, returns nothing, the provider has no queryable endpoint, or the connected server lacks the discovery capability, the editor SHALL fall back to manual model-id entry without failing. An existing provider SHALL be editable in place: for an api provider the editor SHALL prompt to change its base_url and api key (its name fixed), and for an `oauth:codex` provider the editor SHALL offer that provider's OAuth actions: sign in, sign out, and switch account (switch being sign out then sign in). Because the OAuth sign-in flow is asynchronous, opens a browser, and needs the plain terminal, the editor SHALL run those OAuth actions by saving the current config, leaving the full-screen editor, performing the sign-in or sign-out for the selected provider, and then reopening the editor, never attempting the browser flow inside the full-screen UI. All edits SHALL go through the same validation and atomic write as the non-interactive provider commands.

#### Scenario: model selection lists the chosen API provider's models

- **WHEN** the user sets a model role, selects an api provider, and that provider's `/models` endpoint responds
- **THEN** the editor lists that provider's model ids for the user to search and pick, and does not mix in other providers' models

#### Scenario: model selection lists the chosen OAuth provider's models

- **WHEN** the user sets a model role, selects a signed-in `oauth:codex` provider in the remote Server region, and the server supports provider-model discovery
- **THEN** the editor lists the model ids returned for that provider and does not require a provider `base_url`

#### Scenario: model fetch failure degrades to manual entry

- **WHEN** the provider model endpoint or server discovery operation cannot be reached, returns nothing, returns an error, or the server lacks the required capability
- **THEN** the editor lets the user type the model id instead, and displays a fallback reason without crashing

#### Scenario: an existing API provider is editable

- **WHEN** the user selects an existing api provider and chooses edit
- **THEN** the editor prompts to change its base_url and api key and saves through the same validation and atomic write as the non-interactive commands

#### Scenario: an OAuth provider offers sign-in actions

- **WHEN** the user selects an existing `oauth:codex` provider and chooses edit
- **THEN** the editor offers sign in, sign out, and switch account for that provider

#### Scenario: an OAuth action leaves and re-enters the editor

- **WHEN** the user chooses sign in, sign out, or switch for an `oauth:codex` provider
- **THEN** the editor saves the config, leaves the full-screen UI, runs that provider's sign-in or sign-out, and then reopens the editor with the result shown; the browser flow never runs inside the full-screen UI


<!-- @trace
source: oauth-provider-status-and-model-discovery
updated: 2026-07-13
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-cli/src/config.rs
  - docs/env.md
  - README.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/main.rs
  - docs/design-cli-config.md
-->

---
### Requirement: The key hints stay visible

Every interactive config screen — the top-level menu, the three-region panel, and the provider editor (browse, the add-provider wizard, the set-model wizard, the timezone picker, and the edit and OAuth-action flows) — SHALL render the key hints (including how to go back and how to quit) on a line separate from the transient status/result message, so that performing an action and showing its result does not overwrite or hide the hints.

#### Scenario: hints survive an action's output

- **WHEN** the user performs an action that prints a result/status (for example adding a provider, which prints "added provider 'X'")
- **THEN** the key hints (back / quit / navigation) remain visible on their own line rather than being overwritten by the result

#### Scenario: every config screen keeps its hints

- **WHEN** any of the config screens (menu, three-region panel, provider editor and its wizards, timezone picker) is showing
- **THEN** its key hints are on a line that action or status output cannot overwrite

<!-- @trace
source: per-provider-codex-oauth
updated: 2026-07-12
code:
  - docs/env.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/config_panel.rs
  - docs/design-cli-config.md
  - README.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-cli/src/auth.rs
-->

---
### Requirement: Daemon and server regions persist only through their owners

The Daemon and Server panel regions SHALL keep independent availability, revision, snapshot entries, staged changes, and apply targets. A daemon edit SHALL be sent to fleetyd and a server edit SHALL be sent to fleety-server. If an owner is unavailable, its region SHALL display an unavailable state and SHALL NOT offer a direct-file fallback. After the user saves a different current connection profile, the panel SHALL close the previous connection, discard both remote regions' prior snapshot, revision, and staged changes, connect using the newly selected profile, and reload the Server and current-device Daemon snapshots before either region can apply a change. A failed reconnect SHALL leave both remote regions unavailable and SHALL NOT restore or reuse the previous connection.

#### Scenario: daemon unavailable leaves other regions usable

- **GIVEN** the server connection works but fleetyd for the current device is not connected
- **WHEN** the panel opens
- **THEN** Connection, CLI, and Server remain usable while Daemon is marked unavailable

#### Scenario: server unavailable does not convert remote edits to local writes

- **GIVEN** the server cannot be reached
- **WHEN** the panel opens
- **THEN** Connection and CLI remain usable, Daemon and Server are marked unavailable, and no remote setting is written locally

#### Scenario: staged changes remain separated

- **WHEN** the user stages one daemon setting and one server setting
- **THEN** applying in either region sends only that region's changes and revision to its owner

#### Scenario: saved profile switch reconnects before remote use

- **GIVEN** the panel is connected to server B and profile A identifies a different server
- **WHEN** the user selects profile A as current and saves the Connection region
- **THEN** the panel closes the B connection, connects using profile A, and reloads A's Server and current-device Daemon snapshots before enabling either remote apply action

#### Scenario: old remote state is not carried to the new server

- **GIVEN** the panel has snapshot entries, revisions, and staged changes from server B
- **WHEN** the user saves profile A as current
- **THEN** all B-derived Server and Daemon state is discarded and no B-derived change can be sent through the A connection

#### Scenario: reconnect failure cannot fall back to the old server

- **GIVEN** the panel is connected to server B and the newly saved profile A cannot complete its connection and Hello handshake
- **WHEN** the profile switch is attempted
- **THEN** the B connection remains closed, Server and Daemon are unavailable, Connection and CLI remain usable, and no remote config file is modified

#### Scenario: daemon refresh failure does not hide a usable server

- **GIVEN** profile A connects and returns a Server snapshot but the current device daemon is unavailable on A
- **WHEN** the panel refreshes both remote regions
- **THEN** the Server region is usable with A's state and the Daemon region is unavailable

<!-- @trace
source: reconnect-config-panel-profile-switch
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config_panel.rs
-->

---
### Requirement: OAuth model discovery uses the authenticated backend identity

When the connected server discovers models for a signed-in `oauth:codex` provider, it SHALL request the Codex model catalog with the provider's bearer and account identity and a backend-compatible originator. The default backend originator SHALL be `codex_cli_rs`, while the OAuth authorization URL SHALL continue to use `fleety` by default. An explicit `FLEETY_CODEX_ORIGINATOR` override SHALL apply to authenticated Codex backend requests without moving credentials to the CLI.

#### Scenario: signed-in provider receives its model catalog

- **GIVEN** an `oauth:codex` provider is configured and signed in on the connected server
- **WHEN** the user opens model selection for that provider
- **THEN** the server requests the catalog with the OAuth bearer, account ID, client version, and `codex_cli_rs` originator and returns the model IDs to the CLI

#### Scenario: authorize flow keeps its own identity

- **WHEN** Fleety constructs the OAuth authorization URL without an originator override
- **THEN** the URL contains `originator=fleety` and the later authenticated catalog request uses `originator: codex_cli_rs`

#### Scenario: credentials remain server-owned

- **WHEN** the CLI requests model discovery for the signed-in provider
- **THEN** only model IDs or a sanitized error cross the protocol and no OAuth credential is read from or written to a client-side fallback file

<!-- @trace
source: fix-codex-model-catalog-originator
updated: 2026-07-14
code:
  - docs/env.md
  - crates/fleety-tools/src/oauth.rs
-->

---
### Requirement: Settings use owner-aware navigation and state

The Settings route SHALL provide Connection, CLI, Daemon, Server, and Providers & Models pages. Each page SHALL identify the selected profile and destination owner and SHALL represent Loading, Available, Dirty, Applying, Conflict, Failed, and Unavailable states explicitly. Storage filenames SHALL NOT be used as the primary page title or save destination.

#### Scenario: provider page names the connected Server

- **WHEN** the user opens Providers & Models while profile `office` is connected
- **THEN** the page SHALL identify `office` and its Server endpoint and SHALL NOT describe the action as editing `providers.toml`


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
### Requirement: Settings stage and apply changes per owner

CLI, Daemon, Server, and Provider/Model edits SHALL be staged before persistence. Apply SHALL act on exactly one owner, use that owner's persistence path, and report Saved, Restart required, Conflict, or Failed. Dirty state from separate owners SHALL remain separate and SHALL NOT be presented as one atomic transaction.

#### Scenario: failed remote apply retains the edit

- **WHEN** a Server apply fails or conflicts
- **THEN** its staged values SHALL remain Dirty or Conflict, the error SHALL remain visible, and no CLI or Daemon file SHALL be modified

##### Example: stale Server revision

- **GIVEN** Server revision `r1` is staged while the owner has advanced to `r2`
- **WHEN** Apply returns a typed conflict
- **THEN** the staged `r1` edits and remediation remain visible, and CLI and Daemon bytes remain unchanged


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
### Requirement: Profile switching resolves dirty remote state before reconnect

Switching profiles while Server or Daemon state is dirty SHALL require Apply, Discard, or Cancel and SHALL identify the old profile. Apply must succeed before switching; Discard SHALL clear only old-profile staged remote state. After selection, the old transport SHALL close and fresh Server and Daemon snapshots SHALL load from the selected profile.

#### Scenario: cancel keeps profile and edits

- **GIVEN** profile `A` has dirty Server settings
- **WHEN** the user selects profile `B` and chooses Cancel
- **THEN** profile `A` SHALL remain selected, its staged changes SHALL remain, and no reconnect SHALL occur

#### Scenario: failed new connection never reuses old snapshots

- **WHEN** the user discards old staged state, selects profile `B`, and `B` cannot connect
- **THEN** remote pages SHALL become Unavailable and SHALL NOT display or apply profile `A` snapshots

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