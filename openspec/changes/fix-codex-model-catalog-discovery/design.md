## Context

The server-owned Codex OAuth path already follows the current Codex catalog shape: an authenticated `GET /models?client_version=...` request and a top-level `models` array whose entries expose `slug` with `id` as a fallback. The request currently substitutes `agent_core::VERSION`, which is Fleety's workspace release version, for the Codex client compatibility version. The provider screen also renders `catalog=Ready` before any catalog request has succeeded, while every successful-but-unusable JSON response collapses into one error.

The exact production response behind the reported screenshot was not captured, so version filtering is a strongly supported cause, not a proven single cause. The implementation must both remove the verified version-domain error and preserve enough sanitized diagnostics to distinguish any remaining upstream contract drift.

## Goals / Non-Goals

**Goals:**

- Send a Codex-compatible client version independent of the Fleety release version.
- Keep model IDs dynamic and server-fetched with the selected provider's OAuth credential.
- Distinguish missing, empty, and unusable successful catalog responses without leaking secrets.
- Make the provider screen distinguish catalog queryability from successful loading.
- Preserve manual model-ID entry whenever discovery is unavailable.

**Non-Goals:**

- Hard-coding a static list of Codex model IDs.
- Requiring the Codex CLI, reading its local cache, or starting Codex app-server.
- Adding a public config knob, dependency, protocol revision, or client-side OAuth access.
- Changing API-provider `/models` behavior.

## Decisions

### Separate Codex catalog compatibility from Fleety versioning

Introduce one private `CODEX_CATALOG_CLIENT_VERSION` value for the Codex backend contract. Use it for the `client_version` query and any request metadata that claims Codex-client compatibility; retain the Fleety version only where the request identifies Fleety itself. Initialize and document the value against a currently supported Codex release during implementation. Reusing `agent_core::VERSION` was rejected because Fleety's semver does not describe Codex compatibility. Discovering a local `codex` executable version was rejected because the server must work without that executable.

### Keep direct authenticated catalog discovery

Retain the existing server-side OAuth request to the Codex backend. Official Codex uses the same `/models` request contract, and this preserves Fleety's credential ownership and deployment model. Switching to Codex app-server was rejected because it adds a runtime process and a second auth/config source solely to obtain IDs.

### Classify successful catalog failures before flattening IDs

Parse the top-level response structure before collecting IDs. Return distinct sanitized errors for a missing or non-array `models` field, an empty array, and a non-empty array with no non-blank `slug` or `id`. Continue ordered de-duplication for usable IDs. Raw bodies and credentials never enter user-visible errors.

### Distinguish queryable and loaded catalog states

Replace the pre-fetch `catalog=Ready` label with `catalog=Queryable` when the provider and negotiated protocol permit a request. Show a loaded state only after a non-empty catalog result exists in the current editor flow. Failures continue to appear in the existing status area and lead to manual entry.

### Verify the upstream boundary with a controlled contract fixture

Tests must assert the exact catalog path, compatibility query, authenticated headers, response classification, ordered de-duplication, and redaction. A fixture that returns models only at or above a minimum client version proves that Fleety's own version can no longer control upstream eligibility.

## Implementation Contract

- **Request:** A signed-in `oauth:codex` provider causes the server to request `{backend}/models` with bearer and account headers from that provider and a dedicated Codex catalog compatibility version. The request does not use the Fleety package version as `client_version`.
- **Response:** A top-level `models` array yields non-blank `slug` values, falling back to non-blank `id`, in source order with duplicates removed.
- **Diagnostics:** HTTP failures retain sanitized status/detail handling. Successful JSON distinguishes: missing or wrong-type `models`, empty `models`, and non-empty `models` with no usable IDs. Diagnostics expose no raw response body, bearer token, refresh token, or account secret.
- **UI:** Before loading, an eligible provider is labeled `catalog=Queryable`, not `catalog=Ready`. A non-empty result can be labeled loaded; any failure shows the sanitized reason and offers manual model-ID entry.
- **Acceptance criteria:** Controlled HTTP tests assert the request version is independent of `agent_core::VERSION`, version-gated fixture models are returned, all three unusable-success classes differ, secret-redaction tests pass, provider TUI assertions distinguish Queryable from loaded, and existing API-provider and fallback tests remain green.
- **Scope boundaries:** No public protocol shape, config schema, API-provider request, persisted model list, or local Codex installation contract changes.

## Risks / Trade-offs

- [Risk] A pinned compatibility version eventually falls below a future upstream minimum. → Mitigation: keep one documented constant and a release-time maintenance note; a distinct diagnostic will make recurrence identifiable.
- [Risk] Upstream changes the private catalog schema. → Mitigation: classify structural failures and keep dynamic parsing isolated behind controlled fixtures.
- [Risk] Request metadata impersonates Codex or misstates Fleety. → Mitigation: use the compatibility value only for fields with Codex compatibility semantics and retain Fleety identity where the header identifies the caller.
- [Trade-off] Direct backend discovery follows an upstream implementation contract rather than the app-server API. → Mitigation: avoid a new runtime dependency now; reconsider app-server only if Fleety adopts it for broader Codex integration.

## Migration Plan

No persisted data migration is required. Deploy the server change and CLI label together. Rollback restores the prior request metadata and label without altering credentials or configuration.

## Open Questions

None. The apply phase must verify the supported compatibility value against the current official Codex release before encoding it.
