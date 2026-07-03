# codex-oauth Specification

## Purpose

TBD - created by archiving change 'codex-oauth'. Update Purpose after archive.

## Requirements

### Requirement: ChatGPT login uses a PKCE authorization-code flow

The CLI SHALL provide a `login` command that authenticates against ChatGPT using an OAuth 2.0 authorization-code flow with PKCE (S256). It SHALL generate a high-entropy code verifier and its S256 challenge, open the authorization endpoint in a browser with the client id, a loopback redirect URI, a state value, and the code challenge, and capture the returned authorization code on a temporary local loopback listener. It SHALL verify the returned state before exchanging the code, and SHALL exchange the code plus verifier at the token endpoint for an access token and a refresh token. When no browser is available, the command SHALL be able to print the authorization URL instead of opening a browser.

#### Scenario: successful login stores tokens

- **WHEN** a user runs the login command and completes authorization in the browser
- **THEN** the CLI captures the code on the loopback listener, exchanges it with the code verifier, and stores the resulting access and refresh tokens

#### Scenario: state mismatch aborts

- **WHEN** the redirect returns a state value that does not match the one sent
- **THEN** the CLI aborts without exchanging the code and reports an actionable error

#### Scenario: no browser prints the URL

- **WHEN** login is run with the no-browser option
- **THEN** the CLI prints the authorization URL for the user to open manually and still captures the code on the loopback listener


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
### Requirement: OAuth tokens are stored protected and refreshed automatically

The system SHALL persist the access token, refresh token, and expiry in an Agent-side token store file with restricted permissions (mode 0600 on Unix), never in the general config or providers file. Before a model call that uses OAuth, the system SHALL return a valid bearer, refreshing the access token via the refresh token when it is at or near expiry. A refresh failure SHALL return an actionable error asking the user to log in again, and SHALL NOT crash. Login and logout SHALL be recorded in the audit log.

#### Scenario: expired token is refreshed before use

- **WHEN** a model call needs an OAuth bearer and the stored access token is at or past its expiry
- **THEN** the system refreshes it with the refresh token, persists the new token, and uses it for the call

#### Scenario: refresh failure is actionable

- **WHEN** the refresh token is no longer valid and a refresh is attempted
- **THEN** the call returns an actionable error telling the user to log in again, without crashing

#### Scenario: tokens are not world-readable

- **WHEN** tokens are persisted on a Unix host
- **THEN** the token store file is created with owner-only permissions and is not written into the config or providers file


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

The CLI SHALL provide a status command that reports whether the user is logged in and when the token expires without printing the token values, and a logout command that removes the stored tokens. The endpoints, client id, and backend base URL SHALL be overridable by configuration with known public defaults.

#### Scenario: status hides token values

- **WHEN** a user runs the status command while logged in
- **THEN** it reports the logged-in state and expiry without printing the access or refresh token

#### Scenario: logout removes tokens

- **WHEN** a user runs the logout command
- **THEN** the stored token file is removed and subsequent OAuth calls report a logged-out state

#### Scenario: endpoints are overridable

- **WHEN** the authorization endpoint, token endpoint, client id, or backend base URL is set in configuration
- **THEN** the login flow and provider use the configured values instead of the defaults

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