## MODIFIED Requirements

### Requirement: The server exposes authenticated provider model discovery

The server SHALL accept a provider-model discovery request for a named configured provider when the negotiated config protocol supports the operation. For an API provider, it SHALL query the configured `base_url/models` endpoint with the configured API key and return ordered, de-duplicated model IDs. For an `oauth:codex` provider, it SHALL use that provider's server-side OAuth credential, refresh it through the existing credential path when necessary, query the Codex backend model catalog with a dedicated Codex catalog compatibility version that is independent of the Fleety package version, and return ordered, de-duplicated IDs parsed from non-blank model `slug` values or non-blank `id` fallback values. The server SHALL classify a successful response with a missing or non-array `models` field, an empty `models` array, and a non-empty `models` array with no usable IDs as distinct sanitized failures. The server SHALL return model IDs only and SHALL NOT return access tokens, refresh tokens, account secrets, raw catalog bodies, or token-bearing error text.

#### Scenario: signed-in OAuth discovery returns catalog IDs

- **WHEN** a client requests models for a configured signed-in `oauth:codex` provider
- **THEN** the server queries the authenticated Codex catalog with the dedicated Codex compatibility version and returns its non-empty model IDs in source order without token material

#### Scenario: Fleety version does not filter Codex models

- **GIVEN** the upstream catalog returns models only when `client_version` meets its Codex minimum
- **WHEN** the Fleety package version is lower than that minimum but the dedicated Codex compatibility version meets it
- **THEN** the request uses the dedicated Codex compatibility version and returns the eligible model IDs

#### Scenario: OAuth discovery refreshes an expiring credential

- **WHEN** a client requests models for an `oauth:codex` provider whose access token is expired or near expiry and its refresh token is valid
- **THEN** the server refreshes and persists the provider credential through the existing OAuth path before returning model IDs

#### Scenario: API discovery remains compatible

- **WHEN** a client requests models for an API provider with a configured base_url
- **THEN** the server uses the API provider's existing `/models` response forms and returns model IDs without changing API-provider behavior

#### Scenario: missing OAuth credential is actionable

- **WHEN** a client requests models for an `oauth:codex` provider without a usable server-side credential
- **THEN** the server returns an error result that identifies the provider as not signed in and the client can fall back to manual model entry

#### Scenario: missing models field is diagnosed safely

- **WHEN** the upstream catalog returns successful JSON without an array-valued `models` field
- **THEN** the server returns a sanitized structural-catalog error and the client falls back to manual model entry

#### Scenario: empty model array is diagnosed safely

- **WHEN** the upstream catalog returns a successful empty `models` array
- **THEN** the server returns a sanitized empty-catalog error and the client falls back to manual model entry

#### Scenario: unusable model entries are diagnosed safely

- **WHEN** the upstream catalog returns a non-empty `models` array whose entries contain no non-blank `slug` or `id`
- **THEN** the server returns a sanitized unusable-entry error and the client falls back to manual model entry

## ADDED Requirements

### Requirement: Catalog status distinguishes queryability from loaded data

The remote provider editor SHALL render `catalog=Queryable` when a configured provider and negotiated protocol allow a model discovery request but no non-empty result has been loaded. It SHALL NOT render `catalog=Ready` as a synonym for queryability. It SHALL identify the catalog as loaded only after the current editor flow receives a non-empty model list. Discovery failure SHALL display the sanitized fallback reason and SHALL preserve manual model-ID entry.

#### Scenario: eligible provider is queryable before fetch

- **WHEN** a provider is eligible for model discovery and no non-empty result has been received
- **THEN** its row displays `catalog=Queryable` and does not claim the catalog is ready or loaded

#### Scenario: successful fetch establishes loaded state

- **WHEN** model discovery returns at least one usable model ID in the current editor flow
- **THEN** the editor identifies the catalog as loaded and presents the returned models

##### Example: one returned Codex model

- **GIVEN** the catalog response contains `models: [{"slug":"gpt-5-codex"}]`
- **WHEN** the current editor flow receives the parsed ID `gpt-5-codex`
- **THEN** the editor marks the catalog loaded and offers `gpt-5-codex`

#### Scenario: failed fetch preserves manual entry

- **WHEN** model discovery fails with a sanitized catalog diagnostic
- **THEN** the editor shows that reason and permits manual model-ID entry without marking the catalog loaded

##### Example: empty catalog fallback

- **GIVEN** the catalog response contains `models: []`
- **WHEN** discovery returns the sanitized empty-catalog diagnostic
- **THEN** the editor shows that diagnostic, does not mark the catalog loaded, and offers manual model-ID entry
