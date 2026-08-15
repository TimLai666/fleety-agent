## Why

When the cheap tier is unset, Fleety deliberately aliases the auto-review provider to the main tier, but the audit record still hard-codes `provider_model` as `cheap`. This makes the audit history claim that a cheap reviewer was used when the main reviewer actually handled the decision.

## What Changes

- Record the resolved reviewer tier label in auto-review audit entries.
- Report `main` when the `cheap` selector falls back to the main provider, while preserving `cheap` for an explicitly configured cheap tier.
- Keep provider fallback, approval decisions, redaction, and fail-closed behavior unchanged.
- Add regression coverage for the fallback label and update the audit-history specification.

## Capabilities

### New Capabilities

### Modified Capabilities

- `audit-history`: Require auto-review entries to identify the resolved reviewer provider/model label rather than the requested tier name.

## Impact

- Affected specs: `audit-history`
- Affected code:
  - Modified: crates/fleety-server/src/auto_review.rs
  - Modified: crates/fleety-server/src/providers.rs
