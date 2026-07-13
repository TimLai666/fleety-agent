# cli-workflow-integrity Specification

## Purpose

TBD - created by archiving change 'harden-cli-workflow-integrity'. Update Purpose after archive.

## Requirements

### Requirement: Command parsing is strict and lossless

Every CLI command SHALL consume all arguments. Missing flag values, invalid numeric values, unknown flags, and unexpected trailing arguments SHALL fail before I/O. Help SHALL be distinct from usage failure. Ask SHALL preserve every non-flag positional word in order.

#### Scenario: ask preserves multiple words

- **WHEN** the user runs fleety ask hello world
- **THEN** the user message text is exactly hello world

#### Scenario: attachment flag requires a path

- **WHEN** the user runs fleety ask hello --file
- **THEN** the command exits non-zero before connecting

#### Scenario: server verbs reject trailing input

- **WHEN** the user runs fleety server list garbage or misspells --force
- **THEN** the command exits non-zero and connections.toml is unchanged

#### Scenario: invalid numeric input is rejected

- **WHEN** a resume, audit, conversation, or limit argument is not a valid integer
- **THEN** the command reports the invalid value instead of substituting zero or no limit


<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Remote and protocol failures are process failures

ServerMsg Error, an expected business reply with ok false, premature connection close, and malformed JSON payload SHALL return a non-zero exit status. Malformed list payload SHALL NOT be rendered as an empty list.

#### Scenario: server error is non-zero across commands

- **WHEN** ask, voice, resume, audit, rollback, or config receives ServerMsg Error
- **THEN** the actionable message is printed and the process exits non-zero

#### Scenario: rollback rejection is non-zero

- **WHEN** RollbackResult has ok false
- **THEN** rollback apply exits non-zero

#### Scenario: malformed list payload is surfaced

- **WHEN** conversations, audit, or rollback list receives malformed JSON
- **THEN** the command reports a protocol error and exits non-zero instead of showing no entries


<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Pair and init update only a verified profile

Pair SHALL persist credentials only to the named profile that supplied the connection. A direct URL override SHALL NOT write credentials into the current profile. Init SHALL persist and select its proposed profile only after a valid Welcome. Failed interactive init SHALL return non-zero and SHALL use existing command names in remediation.

#### Scenario: named override pairs the named profile

- **GIVEN** profile A is current and profile B is selected with -s B
- **WHEN** pairing succeeds
- **THEN** B receives the token and fingerprint and A is unchanged

#### Scenario: URL override does not poison current

- **WHEN** pairing uses a direct --url override without a named profile
- **THEN** the command requires explicit persistence direction and does not modify the current profile

#### Scenario: failed init is transactional

- **GIVEN** connections.toml bytes are recorded
- **WHEN** init cannot complete Welcome
- **THEN** the command exits non-zero and the recorded bytes are unchanged

#### Scenario: interactive init remediation names pair-code

- **WHEN** guided init requires a pairing code or rejects an invalid selection
- **THEN** it identifies the valid range and refers to fleety pair-code


<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: OAuth remains bound to its preflight server

OAuth login SHALL retain the preflight resolved target for credential delivery and SHALL verify the same server identity after browser authorization. The callback listener SHALL have a finite deadline and SHALL return an actionable timeout error.

#### Scenario: current profile changes during browser wait

- **GIVEN** OAuth preflight selected server A
- **WHEN** another process selects server B before the callback arrives
- **THEN** the credential is delivered only to verified server A and never to B

#### Scenario: server identity changes during login

- **WHEN** callback completion reconnects to an endpoint with a different fingerprint
- **THEN** credential delivery is refused

#### Scenario: callback never arrives

- **WHEN** no callback is received before the deadline
- **THEN** login exits non-zero with retry guidance


<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Provider editor distinguishes staged and saved state

Provider and model mutations SHALL mark the editor dirty and SHALL be described as staged until save succeeds. Leaving a dirty editor SHALL require Save, Discard, or Cancel. An OAuth action SHALL run only after SaveOutcome Saved; conflict or save error SHALL retain staged changes and SHALL NOT start OAuth.

#### Scenario: quitting dirty editor requires a decision

- **WHEN** the user changes a provider or model and presses q
- **THEN** the editor asks to Save, Discard, or Cancel instead of silently closing

#### Scenario: save failure blocks OAuth

- **WHEN** an OAuth action requires save and the save returns conflict or error
- **THEN** the browser flow does not start and the dirty edit remains available

#### Scenario: config menu labels match destinations

- **WHEN** the top config menu is shown
- **THEN** it presents one truthful Providers and Models destination instead of two labels that open the same screen


<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: ACP preserves connection credentials and editor binding

ACP SHALL use the resolved profile URL and token. Resolver failure SHALL NOT fall back to localhost. Unknown ACP verbs SHALL fail instead of starting the adapter. Refresh SHALL preserve an existing server env binding unless an explicit replacement is supplied. Zed settings SHALL be atomically replaced.

#### Scenario: ACP uses paired profile token

- **WHEN** an editor launches ACP with a paired current profile
- **THEN** ACP Hello includes that profile token

#### Scenario: refresh preserves server

- **GIVEN** the installed Fleety editor entry contains FLEETY_AGENT_URL
- **WHEN** update refreshes only the executable path
- **THEN** the existing FLEETY_AGENT_URL remains unchanged

#### Scenario: resolver error is not localhost

- **WHEN** ACP cannot parse the configured connections file
- **THEN** it exits with that error and does not connect to localhost


<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Service and update commands have bounded effects

fleetyd and fleety-server SHALL enter runtime only with no command or their explicit service entry. Help SHALL exit zero without starting runtime and unknown verbs SHALL exit non-zero. fleety daemon up and down SHALL map to start and stop. fleety update SHALL fail if a required fleetyd update child exits non-zero.

#### Scenario: service help never starts runtime

- **WHEN** fleetyd --help or fleety-server --help runs
- **THEN** usage is printed and no service loop starts

#### Scenario: unknown service verb fails

- **WHEN** fleetyd statuz or fleety-server statuz runs
- **THEN** it exits non-zero and no foreground server or daemon starts

#### Scenario: fleetyd update failure propagates

- **WHEN** the delegated fleetyd update exits non-zero
- **THEN** fleety update reports the component failure and exits non-zero


<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Help and credential writes have no hidden damage

Top-level help and version SHALL be handled before configuration seeding or legacy migration and SHALL NOT modify user files. Migration errors on executable commands SHALL be surfaced. OAuth token writes SHALL atomically replace an owner-only file and SHALL report permission or replace failures.

#### Scenario: help does not migrate

- **GIVEN** legacy config files exist
- **WHEN** fleety --help runs
- **THEN** no legacy file is created, renamed, or deleted

#### Scenario: migration failure is visible

- **WHEN** an executable command requires migration and migration fails
- **THEN** the command exits non-zero with the migration error

#### Scenario: token replacement is atomic

- **GIVEN** a valid existing token file
- **WHEN** writing a replacement fails before rename
- **THEN** the existing file remains unchanged and login does not report success

<!-- @trace
source: harden-cli-workflow-integrity
updated: 2026-07-14
code:
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/auth.rs
  - docs/roadmap.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-tools/src/config.rs
  - docs/STATUS.md
  - crates/fleety-cli/src/model_picker.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/bridge.rs
  - docs/env.md
  - crates/fleety-tools/src/providers_config.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/provider_tui.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-server/tests/server_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->