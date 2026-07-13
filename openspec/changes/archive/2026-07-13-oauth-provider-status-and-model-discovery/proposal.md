## Why

The OAuth login flow already delivers credentials to the configured Fleety server, but the interactive provider editor only renders static provider configuration. It therefore cannot tell the user whether the server currently has usable OAuth credentials. The editor also assumes every provider has a `base_url` and rejects the OAuth provider before attempting model discovery, producing `fetch failed (provider has no base_url)` even after a successful login.

OpenAI Codex OAuth model discovery is a server-owned authenticated operation. The server has the OAuth refresh token and account context, so the client must not work around this by receiving or handling OAuth tokens locally.

## What Changes

- Show an explicit OAuth authentication state in the interactive provider editor: signed in, not signed in, or unavailable.
- Add a negotiated protocol operation for the client to request model IDs from the server for a named provider.
- Make the server discover regular API models through the configured API endpoint and discover Codex OAuth models through the authenticated Codex backend catalog.
- Make remote `fleety config` use the server-mediated model discovery operation.
- Keep API-provider local discovery behavior and manual model entry as fallbacks.
- Add regression tests for the original no-`base_url` failure, OAuth status rendering, protocol handling, and Codex catalog parsing/authentication.

## Non-Goals

- Do not hardcode a static list of Codex model IDs.
- Do not move OAuth access or refresh tokens from the server to the CLI.
- Do not add model discovery for OAuth kinds other than `oauth:codex`.
- Do not change the existing login, logout, credential storage, or provider selection semantics outside the interactive config editor.
- Do not add an inline authentication query to the local-only editor when the credential authority is a remote server. That editor will state that auth status is unavailable rather than guessing.

## Capabilities

### New Capabilities

- `oauth-provider-model-discovery`: server-mediated authentication status and model catalog discovery for OAuth providers, including the negotiated wire contract and safe fallback behavior.

### Modified Capabilities

- `interactive-config-panel`: provider rows expose current OAuth state when available, and the model wizard uses provider-specific server discovery instead of requiring `base_url`.
- `codex-oauth`: the existing server-side OAuth credentials become the authority for authenticated Codex model catalog requests without exposing token material to the client.

## Impact

- `crates/fleety-protocol`: additive config protocol frames and a config protocol version bump from 3 to 4.
- `crates/fleety-server`: provider model discovery, Codex OAuth catalog request, response parsing, and request validation.
- `crates/fleety-cli`: provider editor state rendering, remote status/model request wiring, and regression tests.
- `crates/fleety-tools`: shared Codex catalog request/parsing support.
- Existing API providers retain their current `/models` discovery path. Old servers remain usable because the client gates the new operation by negotiated protocol version and falls back to manual entry.

