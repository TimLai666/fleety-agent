## Context

The auto-review gate is created from the named provider tiers. It requests the `cheap` tier, and the provider registry intentionally aliases that selector to `main` when no cheap provider is configured. The gate currently writes the requested selector into `provider_model`, so audit history loses the resolved provider identity.

## Goals / Non-Goals

**Goals:**

- Carry the resolved provider-tier label from `ProviderTiers` into `AutoReviewGate`.
- Record `main` when the cheap selector resolves to the main provider and retain `cheap` when a distinct cheap provider resolves.
- Preserve the existing provider fallback, decision protocol, redaction, timeout, and fail-closed behavior.
- Add regression tests that exercise both provider resolution and the emitted auto-review audit metadata.

**Non-Goals:**

- Do not change which provider is selected when the cheap tier is absent.
- Do not expose provider URLs, API keys, prompts, raw arguments, or other secrets in audit records.
- Do not redesign the provider trait to expose a vendor-specific model identifier; this fix reports the resolved registry label already used by the runtime.

## Decisions

### Carry the resolved label alongside the provider

Add a `ProviderTiers::resolve_with_label` operation that returns the selected provider and its canonical registry label. Keep `ProviderTiers::resolve` as a compatibility-preserving provider-only wrapper. The resolver SHALL return the registry label for a direct hit and the stored main fallback label for an unknown selector, so `cheap` resolving to `main` is observable without duplicating fallback logic.

Alternative rejected: infer the label inside `AutoReviewGate` from environment variables. That would diverge from `providers.toml` resolution and could report a label different from the provider actually selected.

### Store the label on the review gate

Add labeled constructor paths for `AutoReviewGate`. `from_tiers` and the allowed-tools scheduler gate SHALL use the provider and label returned by `resolve_with_label`; the existing direct constructors remain available for tests and direct callers with their current default label. Audit creation SHALL read the gate's stored label rather than a hard-coded string.

Alternative rejected: change only the audit serialization layer. The serializer does not know which provider was resolved, so it cannot distinguish a configured cheap provider from a main fallback.

### Preserve the main fallback identity in structured provider configuration

When building `ProviderTiers` from `providers.toml`, store the label of whichever role becomes the main fallback: `main`, then `cheap`, then the first resolvable role according to the existing selection order. This makes unknown-tier resolution and its audit label describe the same provider.

## Implementation Contract

- **Behavior:** An auto-review audit entry reports the canonical resolved reviewer label. With a distinct `cheap` role it reports `cheap`; with no cheap role it reports `main` when the main provider is selected.
- **Interface / data shape:** `ProviderTiers::resolve_with_label` returns the selected `Arc<dyn ModelProvider>` plus a `String` label. `ProviderTiers::resolve` continues returning only the provider. `AutoReviewGate` stores the label and writes it to the existing `provider_model` JSON field. The tier-aware allowed-tools constructor used by scheduler turns SHALL carry the same label.
- **Failure modes:** Missing cheap configuration continues to alias to main. Provider errors, timeouts, invalid reviewer JSON, unsafe redaction, and denied decisions retain their current results and failure categories. No new approval fallback or human-interaction path is introduced.
- **Acceptance criteria:** The provider unit tests prove cheap-unset resolves to the main label and configured cheap resolves to the cheap label. The auto-review unit test proves the audit JSON records `main` for the cheap-unset path. Existing targeted tests, formatting, `git diff --check`, Spectra analysis, and validation pass.
- **Scope boundaries:** Only provider-label propagation across normal and scheduler auto-review gates and its audit-history contract are in scope. Provider selection policy, model calls, audit redaction, storage format outside the existing `provider_model` value, and unrelated provider metadata remain unchanged.

## Risks / Trade-offs

- [Risk] The label identifies the runtime registry tier, not a vendor-specific model ID. → Mitigation: retain the existing `provider_model` field and use the canonical resolved label, which is the identity available at the tier-resolution boundary without exposing provider configuration secrets.
- [Risk] A future provider path may add aliases or pools with labels that are not model IDs. → Mitigation: route all audit labels through the same resolver and preserve the selected registry key rather than reconstructing labels from environment variables.
