## 1. Protocol contract and regression coverage

- [x] 1.1 Implement Decision 1: Add a negotiated provider-model protocol capability with version 4 wire shapes `ProviderModelList` and `ProviderModelListResult`, update every exhaustive `ClientMsg`/`ServerMsg` match and smoke constructor, and verify JSON round-trips plus config protocol negotiation with protocol tests.
- [x] 1.2 Cover the Interface and data shape contract for model IDs, errors, and credential-status display in focused protocol/TUI-facing tests, and implement Decision 5: Keep protocol handling exhaustive, verifying that no credential payload can appear in a provider-model result.

## 2. OAuth catalog support

- [x] 2.1 Implement Decision 3: Keep model discovery server-mediated and provider-specific for server-owned Codex catalog parsing and authenticated fetch so `oauth:codex` reads `models[].slug` with `id` fallback, preserves source order, removes blank/duplicate IDs, and uses the provider's refreshed credential; verify success, malformed, empty, and authentication-failure cases with unit tests.
- [x] 2.2 Verify that Codex model catalog requests use `/models?client_version=<agent version>`, Bearer authentication, and the optional `chatgpt-account-id` header without exposing token material in returned errors or results.

## 3. Server model discovery

- [x] 3.1 Implement The server exposes authenticated provider model discovery for API and `oauth:codex` providers, validating the named configured provider, routing to the correct catalog, and returning ordered de-duplicated model IDs; verify both provider kinds with server tests.
- [x] 3.2 Implement the Failure modes and fallback contract for missing providers, missing OAuth credentials, refresh failures, non-success upstream responses, malformed catalogs, and empty catalogs; verify sanitized `WireError` results and absence of token text.
- [x] 3.3 Verify Decision 3 server-owned provider routing against the existing API `/models` behavior so API providers retain their current key and response-shape compatibility.

## 4. Authentication status in the editor

- [x] 4.1 Implement Decision 2: Reuse credential status as the authentication indicator and Remote provider authentication state is explicit by injecting server credential status into the provider editor and rendering `auth=signed in`, `auth=not signed in`, or `auth=unavailable` without credential values; verify all three states with TUI tests.
- [x] 4.2 Implement the Login status and logout do not leak tokens editor contract so status failures remain editable and local-only editing reports unavailable instead of guessing; verify status rendering and masking tests.

## 5. Provider-specific model wizard

- [x] 5.1 Implement Decision 4: Inject discovery into the existing TUI loop and Guided provider and model editing with an injectable model-fetch callback, routing remote OAuth providers through protocol discovery while retaining direct API discovery, and verify that OAuth selection no longer requires `base_url`.
- [x] 5.2 Verify the Observable behavior contract for signed-in OAuth model lists, empty/error results, old-server fallback, API-provider lists, manual model entry, and visible fallback notes with provider TUI tests.

## 6. Remote config integration and compatibility

- [x] 6.1 Wire remote config status and model requests over the existing structured connection, gate them on config protocol version 4, and verify the Protocol and editor preserve backward-compatible fallback behavior with connection/config tests.
- [x] 6.2 Verify the Failure modes and fallback contract end to end at the editor boundary: status or catalog failures never block saving provider configuration, and old protocol peers never receive an unsupported request.

## 7. Documentation and specification synchronization

- [x] 7.1 Update the interactive-config-panel and codex-oauth documentation surfaces, including the user-facing config guidance, so OAuth auth states, server-mediated model discovery, and manual fallback are documented without promising token access on the CLI; verify all `FLEETY_*` and command documentation remains unchanged unless directly relevant.
- [x] 7.2 Review the Scope boundaries and all parallel protocol surfaces for consistency, then verify `spectra analyze --change oauth-provider-status-and-model-discovery` and `spectra validate --change oauth-provider-status-and-model-discovery --strict` pass.

## 8. Complete verification

- [x] 8.1 Verify the Acceptance criteria with `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings`, recording any environment-only limitations separately from code failures.

