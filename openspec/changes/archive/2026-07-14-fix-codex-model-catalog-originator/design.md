## Context

The OAuth authorize URL and authenticated Codex backend requests use the same client ID but different originator identities. Fleety currently has the correct split in documentation and the Responses provider, but the model catalog accidentally reuses the authorize helper.

## Goals / Non-Goals

**Goals:** make server-owned model discovery use the same backend originator default as Responses while preserving login behavior and the existing override.

**Non-Goals:** change OAuth storage, refresh, protocol frames, model parsing, or API-provider discovery.

## Decisions

### Keep separate defaults for separate request classes

The authorization URL keeps the existing `originator()` helper with the `fleety` default. The authenticated catalog request uses `backend_originator()`, which shares `FLEETY_CODEX_ORIGINATOR` with Responses and otherwise returns `codex_cli_rs`. This is smaller and safer than changing the authorization flow or importing the official Codex HTTP stack.

### Test the emitted HTTP request

The regression test captures the raw request and asserts the client-version query, bearer, account ID, and backend originator together. A parser-only test would not detect this failure because the request is rejected before parsing.

## Implementation Contract

**Observable behavior:** selecting models for a signed-in `oauth:codex` provider receives the server-returned catalog when the backend accepts the valid credential. Authentication and sanitized-error fallback remain server-owned.

**Scope boundaries:** in scope are `crates/fleety-tools/src/oauth.rs`, its tests, `docs/env.md`, and the matching specification. CLI/server protocol, token files, provider config, and API-provider catalogs are out of scope.

## Risks / Trade-offs

- A deployment that requires a non-default backend identity still depends on `FLEETY_CODEX_ORIGINATOR`; retaining the override avoids a compatibility regression.
- This aligns the request identity but cannot make an expired, revoked, or unauthorized account succeed. Those cases continue to return a sanitized discovery error.
