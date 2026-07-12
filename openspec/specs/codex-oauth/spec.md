# codex-oauth Specification

## Purpose

TBD - created by archiving change 'codex-oauth'. Update Purpose after archive.

## Requirements

### Requirement: ChatGPT login uses a PKCE authorization-code flow

The CLI SHALL provide a `login <provider>` command that authenticates a named `oauth:codex` provider against ChatGPT using an OAuth 2.0 authorization-code flow with PKCE (S256). The command SHALL require a provider argument that names an existing `oauth:codex` provider in the connected server's provider config, and SHALL fail with a usage error (naming an example) when the argument is missing, and with a by-name error when the provider does not exist or is not an `oauth:codex` type. It SHALL generate a high-entropy code verifier and its S256 challenge, open the authorization endpoint in a browser with the client id, a loopback redirect URI, a state value, and the code challenge, and capture the returned authorization code on a temporary local loopback listener. It SHALL verify the returned state before exchanging the code, and SHALL exchange the code plus verifier at the token endpoint for an access token and a refresh token. When no browser is available, the command SHALL be able to print the authorization URL instead of opening a browser. The exchanged tokens SHALL be delivered over the authenticated connection to the currently connected server for storage **tagged with the provider name**; the CLI SHALL NOT persist them to a local token file. Before starting the browser flow, login SHALL verify the connected server advertises per-provider credential support (config protocol 3 or newer) and fail up front with an update-the-server error otherwise, opening no browser. A delivery failure (unreachable, unpaired, or refusing server) SHALL fail the login with a remediation message and SHALL NOT fall back to local storage. Re-running login for a provider that already has a stored credential SHALL replace it, switching that provider's account. On successful delivery, the CLI SHALL name the server (profile and URL) and the provider the credential now lives on, and SHALL delete a leftover legacy local token file when one exists.

#### Scenario: login binds tokens to the named provider

- **WHEN** a user runs login for an `oauth:codex` provider and completes authorization in the browser
- **THEN** the CLI captures the code on the loopback listener, exchanges it with the code verifier, delivers the resulting tokens to the connected server tagged with that provider name, and no token file is written on the CLI host

#### Scenario: two providers hold two accounts

- **WHEN** a user logs in provider `a` with one account and provider `b` with a different account
- **THEN** each provider's stored credential is that account's, independent of the other

#### Scenario: re-login switches a provider's account

- **WHEN** a user runs login again for a provider that already has a stored credential, authorizing a different account
- **THEN** that provider's stored credential is replaced with the new account's tokens

#### Scenario: missing or wrong provider argument is actionable

- **WHEN** login is run with no provider argument, or with a name that is not an existing `oauth:codex` provider
- **THEN** the CLI fails with a usage or by-name error and opens no browser

#### Scenario: state mismatch aborts

- **WHEN** the redirect returns a state value that does not match the one sent
- **THEN** the CLI aborts without exchanging the code and reports an actionable error

#### Scenario: no browser prints the URL

- **WHEN** login is run with the no-browser option
- **THEN** the CLI prints the authorization URL for the user to open manually and still captures the code on the loopback listener

#### Scenario: old server is rejected before the browser opens

- **WHEN** login runs while connected to a server that does not advertise per-provider credential support (config protocol below 3)
- **THEN** the CLI fails before opening the browser, telling the user to update the server first

#### Scenario: delivery failure does not store locally

- **WHEN** the token exchange succeeds but the delivery to the server fails
- **THEN** login fails with a remediation message and no token is persisted anywhere

#### Scenario: leftover legacy local file is cleaned up

- **WHEN** login succeeds on a CLI host that still has a legacy local token file from an earlier version
- **THEN** the CLI deletes that stale local file


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
### Requirement: OAuth tokens are stored protected and refreshed automatically

The server SHALL persist each provider's access token, refresh token, and expiry in a **per-provider** token store file keyed by the provider name (mode 0600 on Unix), never in the general config or providers file, and never on the CLI host. On startup the server SHALL delete its legacy global token file if one exists (no migration; each provider re-logs in fresh). Before a model call that uses OAuth for a given provider, the server SHALL return a valid bearer for **that provider**, refreshing its access token via its refresh token when it is at or near expiry and persisting the refreshed token to the same per-provider store. A refresh failure SHALL return an actionable error asking the user to log in again for that provider, and SHALL NOT crash. Credential storage and removal SHALL be recorded in the audit log with the provider named.

#### Scenario: each provider's token is stored separately

- **WHEN** two `oauth:codex` providers each have a credential put
- **THEN** the server writes two distinct per-provider token store files, and neither read nor refresh of one touches the other

#### Scenario: expired token is refreshed before use

- **WHEN** a model call needs an OAuth bearer for a provider and that provider's stored access token is at or past its expiry
- **THEN** the server refreshes it with that provider's refresh token, persists the new token, and uses it for the call

#### Scenario: legacy global token is cleared on upgrade

- **WHEN** the upgraded server starts and a legacy global token file exists
- **THEN** the server deletes it and no provider inherits it (each must log in again)

#### Scenario: refresh failure is actionable

- **WHEN** a provider's refresh token is no longer valid and a refresh is attempted
- **THEN** the call returns an actionable error telling the user to log in again for that provider, without crashing

#### Scenario: tokens are not world-readable

- **WHEN** tokens are persisted on a Unix server host
- **THEN** each per-provider token store file is created with owner-only permissions and is not written into the config or providers file


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
### Requirement: A provider can authenticate with the OAuth token

A provider SHALL support an authentication mode that sources its bearer from **its own** provider-named OAuth token store instead of a static key, selected by configuration and defaulting to the static-key mode. When the OAuth mode is selected, the provider SHALL obtain a valid bearer for its own provider name (refreshing if needed) before each call and use the existing OpenAI-compatible request path against the configured backend base URL. When no authentication mode is configured, existing behavior SHALL be unchanged.

#### Scenario: an OAuth provider uses its own account

- **WHEN** an `oauth:codex` provider's model is called and that provider is logged in
- **THEN** model calls use that provider's own OAuth account bearer, not another provider's, and require no static API key

#### Scenario: default mode is unchanged

- **WHEN** a provider has no authentication mode configured
- **THEN** it uses the static-key path exactly as before

#### Scenario: using an OAuth provider while logged out is actionable

- **WHEN** an `oauth:codex` provider is called but that provider has no valid stored tokens
- **THEN** a model call returns an actionable error instructing the user to log in for that provider, without crashing


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
### Requirement: Login status and logout do not leak tokens

The CLI SHALL provide a `status [<provider>]` command and a `logout <provider>` command. `status <provider>` SHALL report whether the connected server holds that provider's credential and when it expires without printing the token values; `status` with no provider SHALL enumerate the `oauth:codex` providers in the connected server's provider config and report each one's signed-in state and expiry on its own line. `logout <provider>` SHALL remove that provider's credential stored on the connected server. When a legacy local token file exists on the CLI host, status SHALL note that it is no longer used by any flow. The authorization/token endpoints, client id, and backend base URL SHALL be fixed constants in the code (OpenAI's public values, like the loopback port) — they SHALL NOT be configuration keys or environment variables and SHALL NOT appear in `fleety config`.

#### Scenario: status reports one provider's state without token values

- **WHEN** a user runs status for a provider while the connected server holds that provider's credential
- **THEN** it reports the signed-in state and expiry of that provider's server-side credential without printing the access or refresh token

#### Scenario: status with no provider lists every oauth provider

- **WHEN** a user runs status with no provider argument
- **THEN** the CLI lists each `oauth:codex` provider with its signed-in state and expiry, one per line

#### Scenario: logout removes one provider's credential

- **WHEN** a user runs logout for a provider
- **THEN** that provider's token store on the connected server is removed and subsequent OAuth calls for that provider report a logged-out state, while other providers' credentials are untouched

#### Scenario: leftover local file is flagged

- **WHEN** status runs on a CLI host that still has a legacy local token file
- **THEN** the output notes the file is no longer read by any flow and suggests re-running login

#### Scenario: endpoints are fixed, not configurable

- **WHEN** a user inspects or edits configuration (env, `config.toml`, or `fleety config`)
- **THEN** the Codex client id and endpoints are not present as settings — the login flow and provider always use the hardcoded constants


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