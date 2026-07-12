# server-credential-store Specification

## Purpose

TBD - created by archiving change 'codex-oauth-server-side'. Update Purpose after archive.

## Requirements

### Requirement: Credential delivery frames

The wire protocol SHALL provide credential frames distinct from the config key-value surface: a put frame carrying `{kind, provider, payload}` to store a credential on the server, a status frame carrying `{kind, provider}` to query it, and a delete frame carrying `{kind, provider}` to remove it. `kind` SHALL be a string discriminator (initially `codex-oauth`); a request with an unknown kind SHALL be rejected with an actionable error naming the kind. `provider` SHALL be an optional string that names the provider the credential belongs to; for `kind` `codex-oauth` it SHALL be required, and a `codex-oauth` frame with no provider SHALL be rejected with an actionable error telling the user to update their CLI and log in per provider. The server SHALL store, query, and delete the credential in a per-provider store keyed by the provider name, so two providers' credentials are independent. The payload for `codex-oauth` SHALL be the serde shape of the existing OAuth `Tokens` structure (single source of truth — no separate wire structure). A status reply SHALL report only presence, expiry, and a non-secret detail label, and SHALL NOT contain any token value. A put with a payload missing required fields SHALL be rejected without writing anything.

#### Scenario: put stores the credential under its provider

- **WHEN** an authenticated client sends a credential put with kind codex-oauth, a provider name, and a complete Tokens payload
- **THEN** the server persists it to that provider's own protected token store file and replies success

#### Scenario: a codex frame without a provider is rejected

- **WHEN** a credential frame with kind codex-oauth arrives with no provider (an older client)
- **THEN** the server replies with an actionable error telling the user to update the CLI and log in per provider, and stores nothing

#### Scenario: two providers are isolated

- **WHEN** credentials are put for two different provider names and then one is deleted
- **THEN** the deleted provider reports absent while the other still reports present with its own expiry

#### Scenario: unknown kind is rejected

- **WHEN** a credential frame arrives with kind `something-else`
- **THEN** the server replies with an error naming the unsupported kind and stores nothing

#### Scenario: malformed payload is rejected without side effects

- **WHEN** a credential put for codex-oauth with a provider lacks a required token field
- **THEN** the server replies with an error naming what is missing and the token store file is not created or modified

#### Scenario: status never leaks token values

- **WHEN** an authenticated client sends a credential status for a codex-oauth provider while its credential is stored
- **THEN** the reply reports presence and expiry only, with no access or refresh token material


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
### Requirement: Credential writes require authentication and are audited

Credential put, status, and delete SHALL be accepted only on an authenticated (paired) connection. When the server runs with authentication disabled, it SHALL reject credential frames with an error telling the operator to enable authentication and pair the device. Every accepted put and delete SHALL be recorded in the audit log with the kind and the originating device, and never with token values. A failed write (permission or disk error) SHALL be reported to the client with the underlying cause.

#### Scenario: auth-disabled server refuses credential frames

- **WHEN** a credential put arrives at a server running with authentication disabled
- **THEN** the server rejects it with an error naming the remedy (enable auth and pair), and stores nothing

#### Scenario: put and delete are audited

- **WHEN** a credential put and later a credential delete are accepted
- **THEN** the audit log gains one entry per operation carrying the kind and originating device, with no token material


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
### Requirement: Credential capability is version-negotiated

The server SHALL advertise support for per-provider credential frames through the structured-config protocol version in `Welcome`, bumped to `3`. A client SHALL check the advertised version before sending credential frames: against a server advertising a lower version it SHALL fail the credential operation up front with an error telling the user to update the server, and SHALL NOT fall back to storing credentials locally or globally. Older clients that never send credential frames SHALL be unaffected (the bump is additive), and a client that sends a `codex-oauth` frame without a provider (predating per-provider support) SHALL receive the actionable update-your-CLI rejection rather than a silent global write.

#### Scenario: old server yields an actionable version error

- **WHEN** a client attempts a per-provider credential operation against a server advertising config protocol 2 or lower
- **THEN** the operation fails immediately with an error telling the user to update the server, and no credential frame is sent

#### Scenario: new server accepts old non-credential clients

- **WHEN** a client that predates credential frames connects to a server advertising config protocol 3
- **THEN** every existing non-credential frame keeps working unchanged

#### Scenario: new server rejects an old global-credential client

- **WHEN** a client that predates per-provider support sends a codex-oauth credential frame with no provider to a server advertising config protocol 3
- **THEN** the server rejects it with an actionable update-your-CLI message and writes nothing

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