## Problem

A signed-in `oauth:codex` provider can still fail model discovery because Fleety sends the OAuth authorize-flow identity (`originator: fleety`) to the authenticated Codex model catalog. The provider editor then falls back to manual model entry even though valid credentials are present on the connected server.

## Root Cause

The model catalog request and authorization URL share one `originator()` helper. The authorization flow intentionally defaults to `fleety`, while authenticated Codex backend requests require the same `codex_cli_rs` default used by the Responses client.

## Proposed Solution

- Keep the authorize URL's existing `fleety` default and override behavior.
- Give authenticated catalog requests a backend-specific originator that defaults to `codex_cli_rs` and honors `FLEETY_CODEX_ORIGINATOR`.
- Add a request-capture regression test that proves the bearer, account, client-version, and backend originator headers travel together.
- Update the environment documentation to state that the override covers both Responses and catalog requests.

## Non-Goals

- Changing OAuth token storage, refresh behavior, catalog parsing, or provider ownership.
- Adding a direct-file or client-side credential fallback.

## Success Criteria

- A signed-in `oauth:codex` provider sends `originator: codex_cli_rs` by default when requesting its model catalog.
- The OAuth authorize URL continues to identify Fleety as `originator=fleety` by default.
- The existing server-owned discovery path and manual fallback behavior remain unchanged.

## Impact

- Affected specs: interactive-config-panel
- Affected code:
  - Modified: crates/fleety-tools/src/oauth.rs
  - Modified: docs/env.md
  - New: openspec/changes/fix-codex-model-catalog-originator/specs/interactive-config-panel/spec.md
  - Removed: none
