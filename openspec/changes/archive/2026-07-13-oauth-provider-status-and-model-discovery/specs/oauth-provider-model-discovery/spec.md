## ADDED Requirements

### Requirement: Remote provider authentication state is explicit

The remote interactive provider editor SHALL query the connected server for credential status for each `oauth:codex` provider before rendering the provider list. It SHALL render `auth=signed in` when the server reports a present credential, `auth=not signed in` when the server reports no credential, and `auth=unavailable` when the query is unsupported or fails. The editor SHALL NOT render `not signed in` for an unknown or unavailable status, and SHALL NOT render any credential value.

#### Scenario: signed-in state is visible

- **WHEN** the server reports a present credential for the selected `oauth:codex` provider
- **THEN** the provider row contains `auth=signed in`

#### Scenario: signed-out state is visible

- **WHEN** the server reports no credential for the selected `oauth:codex` provider
- **THEN** the provider row contains `auth=not signed in`

#### Scenario: status is unavailable without false certainty

- **WHEN** the server does not support credential status or the status query fails
- **THEN** the provider row contains `auth=unavailable` and the editor remains usable

### Requirement: The server exposes authenticated provider model discovery

The server SHALL accept a provider-model discovery request for a named configured provider when the negotiated config protocol supports the operation. For an API provider, it SHALL query the configured `base_url/models` endpoint with the configured API key and return ordered, de-duplicated model IDs. For an `oauth:codex` provider, it SHALL use that provider's server-side OAuth credential, refresh it through the existing credential path when necessary, query the Codex backend model catalog with the client version, and return ordered, de-duplicated IDs parsed from model `slug` values or `id` fallback values. The server SHALL return model IDs only and SHALL NOT return access tokens, refresh tokens, account secrets, or token-bearing error text.

#### Scenario: signed-in OAuth discovery returns catalog IDs

- **WHEN** a client requests models for a configured signed-in `oauth:codex` provider
- **THEN** the server queries the authenticated Codex catalog and returns its non-empty model IDs in source order without token material

#### Scenario: OAuth discovery refreshes an expiring credential

- **WHEN** a client requests models for an `oauth:codex` provider whose access token is expired or near expiry and its refresh token is valid
- **THEN** the server refreshes and persists the provider credential through the existing OAuth path before returning model IDs

#### Scenario: API discovery remains compatible

- **WHEN** a client requests models for an API provider with a configured base_url
- **THEN** the server uses the API provider's existing `/models` response forms and returns model IDs without changing API-provider behavior

#### Scenario: missing OAuth credential is actionable

- **WHEN** a client requests models for an `oauth:codex` provider without a usable server-side credential
- **THEN** the server returns an error result that identifies the provider as not signed in and the client can fall back to manual model entry

#### Scenario: malformed or empty catalog is safe

- **WHEN** the upstream model catalog is non-successful, malformed, empty, or contains only blank or duplicate IDs
- **THEN** the server returns a sanitized error or empty result and the client falls back to manual model entry without exposing token material

### Requirement: The protocol and editor preserve backward-compatible fallback

The config protocol SHALL expose the provider-model discovery operation as version 4 or newer. A client connected to a server advertising a lower config protocol version SHALL NOT send the new request and SHALL enter manual model entry with an unavailable-capability reason. A provider status failure or model discovery failure SHALL NOT prevent the user from editing or saving other provider configuration.

#### Scenario: old server uses manual model entry

- **WHEN** a remote editor connects to a server advertising config protocol version 3
- **THEN** it renders OAuth auth as unavailable, skips provider-model discovery, and offers manual model entry

#### Scenario: discovery failure does not block editing

- **WHEN** a provider status or model discovery operation fails
- **THEN** the editor shows a fallback reason and continues to permit model or provider edits

