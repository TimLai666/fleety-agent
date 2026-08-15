## 1. Provider label resolution

- [x] 1.1 Add a failing regression test for **Carry the resolved label alongside the provider** that asserts a missing cheap tier resolves to the main provider with label `main` and a distinct cheap tier resolves with label `cheap`; verify the new provider test fails before implementation.
- [x] 1.2 Implement **Carry the resolved label alongside the provider** and **Preserve the main fallback identity in structured provider configuration** so `ProviderTiers::resolve_with_label` returns the selected provider plus its canonical label while `resolve` preserves its existing provider-only behavior; verify the provider resolution regression tests pass.
- [x] 1.3 Add a failing regression test for **Store the label on the review gate** that builds an auto-review gate from a main-only tier registry and asserts the emitted `provider_model` is `main`; verify the new auto-review test fails before implementation.
- [x] 1.4 Implement **Store the label on the review gate** so `AutoReviewGate::from_tiers` carries the resolved label into audit metadata while direct constructor behavior and existing decision paths remain unchanged; verify the auto-review fallback audit test and existing auto-review tests pass.
- [x] 1.5 Add a failing regression test for the scheduler's tier-aware allowed-tools gate that asserts a main fallback is recorded as `provider_model=main`; verify the new scheduler-path test fails before implementation.
- [x] 1.6 Implement the labeled allowed-tools constructor and update the scheduler auto-review call site so scheduled turns carry the resolved reviewer label; verify the scheduler-path test and existing scheduler tests pass.

## 2. Audit contract and full validation

- [x] 2.1 Complete the **List recent audit entries** contract by keeping `provider_model` sanitized and changing its meaning to the canonical resolved reviewer label, including explicit main fallback and distinct cheap scenarios; verify the delta spec content and `spectra analyze fix-audit-provider-model-label --json` report no critical or warning findings.
- [x] 2.2 Verify the implementation contract across formatting, targeted server tests, `git diff --check`, and `spectra validate fix-audit-provider-model-label`; confirm `spectra instructions apply --change fix-audit-provider-model-label --json` reports `all_done`.
