## ADDED Requirements

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
