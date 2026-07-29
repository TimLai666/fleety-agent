# device-enrollment Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: Daemon connection configuration

The daemon SHALL read `FLEETY_AGENT_URL` for the server URL, trying mDNS for 2 seconds before falling back to `ws://127.0.0.1:8787`. From the resolved server host the daemon SHALL derive both the WebSocket endpoint and the HTTP(S) endpoints used by the SSE+POST fallback, so that one configured host serves both transports. The daemon SHALL read a setting to force the SSE+POST transport and a setting to disable the SSE fallback; when neither is set, it tries WebSocket first and falls back to SSE. It SHALL read `FLEETY_DEVICE_ID` for this device's id (default the hostname, falling back to `fleetyd-device`; the value is used verbatim and is not sanitized, so a path-safe id is the operator's responsibility) and `FLEETY_DEVICE_ROOT` for the filesystem root its on-device tools operate within (default the current working directory).

#### Scenario: URL falls back to localhost

- **WHEN** `FLEETY_AGENT_URL` is unset and mDNS finds nothing within 2 seconds
- **THEN** the daemon connects to `ws://127.0.0.1:8787`

#### Scenario: SSE endpoint derived from the same host

- **WHEN** the daemon has resolved a server host and the WebSocket transport is unavailable
- **THEN** it reaches the SSE and POST endpoints on that same host without requiring a separately configured URL


<!-- @trace
source: sse-transport-fallback
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/Cargo.toml
  - README.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/http.rs
  - docs/env.md
-->

---
### Requirement: Token pairing and persistence

`FLEETY_PAIRING_CODE` SHALL, when passed once, enroll a new device: the server mints a token in the `Welcome` message and the daemon writes it to `~/.fleety/fleetyd.token`. On later starts the daemon SHALL load that persisted token. `FLEETY_TOKEN` SHALL override the persisted token when set.

#### Scenario: pairing persists a minted token

- **WHEN** the daemon starts with `FLEETY_PAIRING_CODE` set and no stored token
- **THEN** it receives a minted token in `Welcome` and writes it to `~/.fleety/fleetyd.token` for reuse

<!-- @trace
source: baseline-config-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-discuss.md
  - .spectra.yaml
  - CLAUDE.md
  - .opencode/commands/spectra-ask.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-debug/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
-->

---
### Requirement: Pairing failures surface readable errors

When `fleety pair` receives a reply that is not a successful `Welcome`, the CLI SHALL report a concise, human-readable message describing the failure and the next step, and SHALL NOT print the Debug representation of internal protocol types to the user. A server `Error` reply SHALL surface the server's message; a `Welcome` with no token SHALL explain that pairing requires the server to run in auth-required mode; any other unexpected frame SHALL yield a generic readable message rather than a `{variant:?}` dump.

#### Scenario: unexpected reply is readable

- **WHEN** the server answers a pairing Hello with a frame that is neither a `Welcome` nor an `Error`
- **THEN** the CLI prints a concise, human-readable failure message and exits non-zero, without dumping the Debug form of the internal message type

#### Scenario: server error is surfaced verbatim

- **WHEN** the server answers pairing with an `Error` frame
- **THEN** the CLI reports the server's error message in a readable form, not a Debug dump

<!-- @trace
source: cli-clipboard-acp-polish
updated: 2026-07-10
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/config.rs
  - docs/env.md
  - crates/fleety-server/src/restart_watch.rs
  - Dockerfile
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/privacy.rs
  - scripts/install.sh
  - crates/fleety-cli/src/main.rs
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/clipboard.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Enrollment operates on connection profiles

`fleety init` and `fleety pair` SHALL operate on the connection profile store (`connections.toml`) rather than the flat `config.json` fields. `fleety init <url>` SHALL create or update a named profile (default name `default`) and make it current; `fleety pair <code>` SHALL pair the current profile and write the minted token into that profile. The device identity used during enrollment SHALL come from the shared `device_id` in `connections.toml`, and when migrating an existing device that `device_id` SHALL be preserved (locked to the pre-existing value), so enrollment on an already-known device does not change its identity.

#### Scenario: pairing writes the token into the current profile

- **WHEN** the user runs `fleety pair CODE` against an auth-required server
- **THEN** the minted token is stored on the current profile in `connections.toml`, and a later reconnect authenticates with that token

#### Scenario: enrollment keeps a migrated device's identity

- **WHEN** a device that previously enrolled (has a `device_id` in `config.json`) migrates and re-enrolls
- **THEN** its `device_id` is unchanged, so the server still recognizes it as the same device

<!-- @trace
source: connection-profiles
updated: 2026-07-10
code:
  - crates/fleety-cli/src/server.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Guided first-run init discovers, picks, and pairs

When `fleety init` runs without a URL on a TTY and mDNS is enabled, the CLI SHALL scan the LAN with collecting discovery, present every found Server as a numbered list, and let the user pick one by number. Discovery alone SHALL NOT create an operational session. Empty input SHALL pick the first entry; an out-of-range or non-numeric choice SHALL re-prompt. The profile name SHALL default to the display name unless `--name` overrides it. Only the selected endpoint SHALL be contacted for enrollment. A same-host loopback pick SHALL skip pairing. A LAN pick SHALL reuse a non-empty credential only when the selected profile name and URL exactly match the saved profile; every other LAN pick SHALL require a non-empty pairing code and a `Welcome` carrying a newly minted token before the URL, token, fingerprint, or current selection is persisted. Empty pairing input SHALL fail before connecting and leave `connections.toml` byte-identical. A picked URL that differs from a credentialed profile with the same name SHALL NOT receive the old token. When discovery finds nothing, or stdout is not a TTY, or mDNS is disabled, `fleety init` SHALL print explicit-URL usage guidance. `fleety init <ws-url>` SHALL apply the endpoint-change credential boundary without entering the picker. When it includes `--pairing-code`, it SHALL send neither an existing saved token nor pin and SHALL replace both only after receiving newly minted complete credentials against the unchanged saved generation.

#### Scenario: pick and pair in one flow

- **WHEN** `fleety init` runs on a TTY, the scan lists one Server, and the user picks it and enters a valid pairing code
- **THEN** the profile SHALL be saved as current with the newly minted token and observed fingerprint

#### Scenario: unselected advertisers remain discovery-only

- **WHEN** guided init displays multiple mDNS candidates and the user selects one
- **THEN** every unselected candidate SHALL receive no `Hello`, token, pairing code, profile mutation, or control authority

#### Scenario: a new LAN pick cannot skip pairing

- **GIVEN** the selected profile name has no stored credential
- **WHEN** the user picks the Server and leaves the pairing-code prompt empty
- **THEN** the CLI SHALL fail before sending `Hello` and SHALL NOT save or select the profile

#### Scenario: pairing acknowledgement must mint a credential

- **GIVEN** a new LAN candidate was explicitly selected and a pairing code was supplied
- **WHEN** the candidate replies with `Welcome` but no newly minted token
- **THEN** enrollment SHALL fail and `connections.toml` SHALL remain byte-identical

#### Scenario: same-host loopback remains frictionless

- **WHEN** guided init selects the locally probed loopback Server
- **THEN** it SHALL enroll without a pairing code because the transport peer is same-host trusted

##### Example: local default selection

- **GIVEN** a Server answers at `ws://127.0.0.1:8787`
- **WHEN** guided init selects that locally probed entry
- **THEN** the CLI SHALL send no pairing code, save the verified `local` endpoint, and make it current

#### Scenario: re-running init on the same saved endpoint keeps its token

- **WHEN** the picked Server's profile name and URL equal a saved credentialed profile
- **THEN** the profile SHALL keep its token and become current

##### Example: same office endpoint

- **GIVEN** profile `office` stores URL `ws://office:8787` and token `office-token`
- **WHEN** init selects `ws://office:8787` as `office`
- **THEN** the Hello SHALL carry `office-token` and the saved credential SHALL remain associated with that URL

#### Scenario: re-running init on a changed saved endpoint requires re-pair

- **GIVEN** profile `office` stores URL `ws://old:8787` and token `old-token`
- **WHEN** guided or explicit init selects `ws://new:8787` as `office`
- **THEN** the CLI SHALL send neither `old-token` nor a profile mutation until a supplied pairing code mints a new token

#### Scenario: nothing found falls back to usage

- **WHEN** the scan window ends with no Server discovered
- **THEN** the CLI SHALL say no Server was found and print explicit-URL usage guidance

##### Example: empty LAN scan

- **GIVEN** no local or LAN Server answers during the bounded scan
- **WHEN** guided init completes discovery
- **THEN** it SHALL make no profile mutation and SHALL print `fleety init <ws-url> --name <name> --pairing-code <code>`

#### Scenario: explicit URL skips the picker

- **WHEN** `fleety init ws://host:8787` runs
- **THEN** the CLI SHALL validate and connect to that explicit URL without running guided discovery


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
### Requirement: A pairing code can be minted over the connection

The client SHALL be able to request the connected server to mint a short-lived pairing code, via a `MintPairingCode` request and a reply carrying either the code or an actionable error. The server SHALL mint (through the same store the first-run code and `pair_create` use) only when authentication is required; when authentication is disabled it SHALL reply with an error explaining that pairing codes are not used and how to enable auth. Because a connection only reaches this point after passing Hello (a valid token or same-host loopback trust), no additional privilege check is needed — an unauthenticated LAN peer is already rejected before it can request a code. The CLI SHALL expose this as `fleety pair-code`, printing the minted code and how to redeem it on another device; against a server too old to support the request it SHALL print a version hint.

#### Scenario: minting on an auth-required server

- **WHEN** `fleety pair-code` runs against an auth-required server it can reach (loopback-trusted or token-authenticated)
- **THEN** the server mints a short-lived code and the CLI prints it with `fleety pair <code>` guidance

#### Scenario: minting is refused when auth is disabled

- **WHEN** `fleety pair-code` runs against a server with authentication disabled
- **THEN** the reply carries an error explaining pairing codes are unused and how to enable auth, and no code is printed

#### Scenario: an old server yields a version hint

- **WHEN** the connected server does not support the mint request
- **THEN** the CLI reports that the server is too old and suggests updating it

<!-- @trace
source: enrollment-reconnect-ux
updated: 2026-07-12
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
-->

---
### Requirement: One-shot commands surface authentication rejections readably

The shared connect-and-hello helper used by the one-shot CLI commands (`pair-code`, `status`, `audit`, `rollback`, `conversations`, …) SHALL, when the server rejects the connection with an authentication error (error kind `unauthenticated`), return a concise human-readable message telling the user the device is not paired and how to fix it (`fleety pair <code>`, and that a code can be minted with `fleety pair-code` on the server host) — never the Debug representation of the internal protocol frame. Any other server `Error` SHALL surface the server's message readably, and any other unexpected frame SHALL yield a generic readable message rather than a `{variant:?}` dump. Successful handshakes and non-authentication failures SHALL be unchanged.

#### Scenario: an unpaired one-shot command is readable

- **WHEN** a one-shot command connects to an auth-required server without a valid token and is rejected as `unauthenticated`
- **THEN** it reports that the device is not paired and how to pair, without dumping the Debug form of the internal frame

#### Scenario: other errors still surface the server message

- **WHEN** the server rejects with a non-authentication `Error`
- **THEN** the command reports the server's message readably

<!-- @trace
source: connection-surface-consistency
updated: 2026-07-12
code:
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/config_panel.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->