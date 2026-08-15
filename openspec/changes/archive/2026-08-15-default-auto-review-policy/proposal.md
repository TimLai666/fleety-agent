## Why

Fleety currently falls back to `full_access` when no policy is configured, so a fresh or unconfigured server bypasses the unattended cheap-model review that was added for autonomous operation. The default must match the intended fully autonomous posture: review non-read actions automatically while preserving explicit policy overrides.

## What Changes

- Make `auto_review` the default when `FLEETY_POLICY` is unset or absent from configuration.
- Preserve explicit `full_access` and `require_approval` values as opt-in overrides.
- Make the shared `Policy::default()` resolve to `AutoReview` so fallback construction cannot silently revert to full access.
- Update the server policy resolver, configuration registry, authoritative prompt, user-facing documentation, runtime specs, and regression tests.
- Keep read tools direct, critical actions fully unattended, and reviewer failures fail-closed as already defined by auto review.

## Capabilities

### New Capabilities

### Modified Capabilities

- `runtime-configuration`: Change the unset `FLEETY_POLICY` default from `full_access` to `auto_review`.
- `agent-conduct-policy`: Make auto review the default execution posture while retaining explicit full-access and interactive-approval overrides.
- `auto-review`: Change auto review from opt-in to the default unattended gate.

## Impact

- Affected specs: `runtime-configuration`, `agent-conduct-policy`, `auto-review`
- Affected code:
  - Modified: `crates/agent-core/src/approval.rs`
  - Modified: `crates/fleety-server/src/main.rs`
  - Modified: `crates/fleety-tools/src/config.rs`
  - Modified: `docs/env.md`
  - Modified: `docs/tools.md`
  - Modified: `prompts/policy.md`
  - Modified: `README.md`
