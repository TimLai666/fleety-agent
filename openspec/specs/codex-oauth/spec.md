# codex-oauth Specification

## Purpose

TBD - created by archiving change 'codex-oauth'. Update Purpose after archive.

## Requirements

### Requirement: ChatGPT login uses a PKCE authorization-code flow

The CLI SHALL provide a `login` command that authenticates against ChatGPT using an OAuth 2.0 authorization-code flow with PKCE (S256). It SHALL generate a high-entropy code verifier and its S256 challenge, open the authorization endpoint in a browser with the client id, a loopback redirect URI, a state value, and the code challenge, and capture the returned authorization code on a temporary local loopback listener. It SHALL verify the returned state before exchanging the code, and SHALL exchange the code plus verifier at the token endpoint for an access token and a refresh token. When no browser is available, the command SHALL be able to print the authorization URL instead of opening a browser. The exchanged tokens SHALL be delivered over the authenticated connection to the currently connected server for storage; the CLI SHALL NOT persist them to a local token file. Before starting the browser flow, login SHALL verify the connected server advertises credential support (config protocol 2 or newer) and fail up front with an update-the-server error otherwise. A delivery failure (unreachable, unpaired, or refusing server) SHALL fail the login with a remediation message and SHALL NOT fall back to local storage. On successful delivery, the CLI SHALL name the server (profile and URL) the credential now lives on, and SHALL delete a leftover legacy local token file when one exists.

#### Scenario: successful login delivers tokens to the connected server

- **WHEN** a user runs the login command and completes authorization in the browser
- **THEN** the CLI captures the code on the loopback listener, exchanges it with the code verifier, delivers the resulting tokens to the connected server over the authenticated connection, and no token file is written on the CLI host

#### Scenario: state mismatch aborts

- **WHEN** the redirect returns a state value that does not match the one sent
- **THEN** the CLI aborts without exchanging the code and reports an actionable error

#### Scenario: no browser prints the URL

- **WHEN** login is run with the no-browser option
- **THEN** the CLI prints the authorization URL for the user to open manually and still captures the code on the loopback listener

#### Scenario: old server is rejected before the browser opens

- **WHEN** login runs while connected to a server that does not advertise credential support
- **THEN** the CLI fails before opening the browser, telling the user to update the server first

#### Scenario: delivery failure does not store locally

- **WHEN** the token exchange succeeds but the delivery to the server fails
- **THEN** login fails with a remediation message and no token is persisted anywhere

#### Scenario: leftover legacy local file is cleaned up

- **WHEN** login succeeds on a CLI host that still has a legacy local token file from an earlier version
- **THEN** the CLI deletes the legacy file and tells the user credentials now live on the server


<!-- @trace
source: codex-oauth-server-side
updated: 2026-07-11
code:
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/main.rs
-->

---
### Requirement: OAuth tokens are stored protected and refreshed automatically

The server SHALL persist the access token, refresh token, and expiry in its own token store file with restricted permissions (mode 0600 on Unix), never in the general config or providers file, and never on the CLI host. Before a model call that uses OAuth, the server SHALL return a valid bearer, refreshing the access token via the refresh token when it is at or near expiry and persisting the refreshed token to the same store. A refresh failure SHALL return an actionable error asking the user to log in again, and SHALL NOT crash. Credential storage and removal SHALL be recorded in the audit log.

#### Scenario: expired token is refreshed before use

- **WHEN** a model call needs an OAuth bearer and the stored access token is at or past its expiry
- **THEN** the server refreshes it with the refresh token, persists the new token, and uses it for the call

#### Scenario: refresh failure is actionable

- **WHEN** the refresh token is no longer valid and a refresh is attempted
- **THEN** the call returns an actionable error telling the user to log in again, without crashing

#### Scenario: tokens are not world-readable

- **WHEN** tokens are persisted on a Unix server host
- **THEN** the token store file is created with owner-only permissions and is not written into the config or providers file


<!-- @trace
source: codex-oauth-server-side
updated: 2026-07-11
code:
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/main.rs
-->

---
### Requirement: A provider can authenticate with the OAuth token

A provider SHALL support an authentication mode that sources its bearer from the OAuth token store instead of a static key, selected by configuration and defaulting to the static-key mode. When the OAuth mode is selected, the provider SHALL obtain a valid bearer (refreshing if needed) before each call and use the existing OpenAI-compatible request path against the configured backend base URL. When no authentication mode is configured, existing behavior SHALL be unchanged.

#### Scenario: an OAuth provider uses the logged-in account

- **WHEN** a provider's authentication mode is set to the Codex OAuth mode and the user is logged in
- **THEN** model calls use the OAuth account's bearer token and require no static API key

#### Scenario: default mode is unchanged

- **WHEN** a provider has no authentication mode configured
- **THEN** it uses the static-key path exactly as before

#### Scenario: using an OAuth provider while logged out is actionable

- **WHEN** a provider is set to the OAuth mode but no valid tokens are stored
- **THEN** a model call returns an actionable error instructing the user to log in, without crashing


<!-- @trace
source: codex-oauth
updated: 2026-07-03
code:
  - crates/fleety-server/src/providers.rs
  - docs/env.md
  - crates/fleety-cli/src/auth.rs
  - prompts/memory.md
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-server/src/gc.rs
  - crates/fleety-server/src/conn.rs
  - docs/tools.md
  - crates/fleety-daemon/src/main.rs
  - README.md
  - crates/fleety-cli/src/main.rs
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-tools/src/oauth.rs
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-tools/src/providers_config.rs
-->

---
### Requirement: Login status and logout do not leak tokens

The CLI SHALL provide a status command that reports whether the connected server holds a credential and when it expires without printing the token values, and a logout command that removes the credential stored on the connected server. When a legacy local token file exists on the CLI host, status SHALL note that it is no longer used by any flow. The endpoints, client id, and backend base URL SHALL be overridable by configuration with known public defaults.

#### Scenario: status reports the server-side state without token values

- **WHEN** a user runs the status command while the connected server holds a credential
- **THEN** it reports the signed-in state and expiry of the server-side credential without printing the access or refresh token

#### Scenario: logout removes the server-side credential

- **WHEN** a user runs the logout command
- **THEN** the token store on the connected server is removed and subsequent OAuth calls report a logged-out state

#### Scenario: leftover local file is flagged

- **WHEN** status runs on a CLI host that still has a legacy local token file
- **THEN** the output notes the file is no longer read by any flow and suggests re-running login

#### Scenario: endpoints are overridable

- **WHEN** the authorization endpoint, token endpoint, client id, or backend base URL is set in configuration
- **THEN** the login flow and provider use the configured values instead of the defaults


<!-- @trace
source: codex-oauth-server-side
updated: 2026-07-11
code:
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/main.rs
-->

---
### Requirement: Login fails fast on an unavailable loopback port

Because the Codex OAuth redirect URI is registered to a fixed loopback port, `fleety auth login` SHALL check that the port is available before opening the browser, and when it is already in use SHALL abort with an actionable message that states the fixed-port constraint and how to resolve it (free the port or close a stuck prior login, then retry), instead of sending the user through authorization only to fail at the redirect. The check SHALL NOT print token values and SHALL leave any existing stored tokens untouched.

#### Scenario: busy port aborts before the browser opens

- **WHEN** the fixed OAuth loopback port is already in use and the user runs login
- **THEN** the CLI aborts before opening the browser with an actionable message that explains the fixed-port requirement and how to free the port

#### Scenario: free port proceeds normally

- **WHEN** the fixed OAuth loopback port is available
- **THEN** login opens the browser and captures the authorization code as before

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