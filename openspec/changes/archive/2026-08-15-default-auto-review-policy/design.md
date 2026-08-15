## Context

Fleety already implements an unattended `auto_review` gate for non-read tools, including critical tools, but the runtime and configuration registry still fall back to `full_access`. A fresh server therefore bypasses the intended reviewer unless an operator remembers to set `FLEETY_POLICY` explicitly.

The effective policy has multiple fallback surfaces: the server's environment parser, the shared configuration registry, and the core `Policy::default()` implementation. These surfaces must agree or a missing configuration can silently restore unrestricted execution.

## Goals / Non-Goals

**Goals:**

- Make `auto_review` the effective policy when `FLEETY_POLICY` is unset, empty, absent from config, or unrecognized by the server parser.
- Make the configuration registry advertise `auto_review` as the default.
- Make `Policy::default()` resolve to `AutoReview` for safe in-process fallback construction.
- Preserve explicit `full_access` as the deliberate direct-execution override and `require_approval` as the deliberate interactive override.
- Keep read tools direct, keep critical actions fully unattended under the default, and retain auto review's existing fail-closed behavior.
- Synchronize code comments, authoritative prompts, docs, tests, and runtime specifications.

**Non-Goals:**

- Do not remove or rename `full_access`, `require_approval`, or `auto_review`.
- Do not add a new model provider, change the cheap-provider selection, or change review timeout behavior.
- Do not convert test fixtures that intentionally construct `Policy::FullAccess` into implicit defaults.
- Do not rewrite historical archived change snapshots.

## Decisions

### Policy resolver and registry both default to AutoReview

Change `policy_from_env()` and the `FLEETY_POLICY` registry entry to return `auto_review` when no valid explicit value is present. The registry is the source for config-file seeding and UI snapshots, while the server resolver is the final runtime fallback; changing both prevents config and direct environment startup from diverging.

Alternative: change only the registry. Rejected because a direct server launch with no config would still use `FullAccess`.

### Policy::default() matches runtime fallback

Change the derived core default from `FullAccess` to `AutoReview`. This protects callers that construct a policy through the type's default rather than through the server resolver.

Alternative: leave the core default unchanged and rely on server wiring. Rejected because future or alternate entry points could silently reintroduce unrestricted behavior.

### Explicit policy overrides remain stable

Exact lowercase `full_access` continues to select direct audited execution, and exact lowercase `require_approval` continues to select the interactive gate. Exact lowercase `auto_review` selects the same unattended reviewer as the default. Unknown or mixed-case values do not become a permissive override; they fall back to `AutoReview`.

Alternative: preserve unknown values as `FullAccess` for backward compatibility. Rejected because a typo in a security-sensitive setting must not disable review.

### Documentation and specifications describe the same precedence

Update the authoritative policy prompt, environment reference, tool reference, README, and the three affected specs. Every surface states that `auto_review` is default and that `full_access` requires an explicit setting.

## Implementation Contract

- **Behavior:** With `FLEETY_POLICY` unset or empty and no config value, the server uses `Policy::AutoReview`. Mutate and critical calls therefore enter the existing cheap-model reviewer; reads remain direct.
- **Behavior:** With `FLEETY_POLICY=full_access`, the server uses `Policy::FullAccess`. With `FLEETY_POLICY=require_approval`, it uses `Policy::RequireApproval`.
- **Failure mode:** An unrecognized or mixed-case `FLEETY_POLICY` value falls back to `Policy::AutoReview`; it MUST NOT fall back to `FullAccess`.
- **Interface/data shape:** The accepted registry values remain exactly `full_access`, `require_approval`, and `auto_review`; only the registry default changes to `auto_review`.
- **Acceptance criteria:** Unit tests cover resolver defaults and explicit overrides, core `Policy::default()`, registry default and config resolution. Targeted `agent-core`, `fleety-tools`, and `fleety-server` tests pass, and `spectra validate default-auto-review-policy` passes.
- **Scope boundaries:** In scope are default selection, policy documentation, runtime specs, and regression tests. Out of scope are the reviewer protocol, provider implementation, timeout semantics, critical detectors, and explicit policy behavior.

## Risks / Trade-offs

- [Risk] Fresh deployments now make a cheap-model call before every mutate or critical tool. → Mitigation: document `FLEETY_POLICY=full_access` as an explicit override and retain the positive review timeout.
- [Risk] An unavailable or unconfigured cheap provider can deny mutations by default. → Mitigation: this is the existing fail-closed auto-review contract; startup and docs identify the explicit override for operators who intentionally accept direct execution.
- [Risk] Existing tests or fixtures may accidentally rely on implicit full access. → Mitigation: update only tests that assert default selection and leave explicit `Policy::FullAccess` fixtures unchanged.

## Migration Plan

No data migration is required. On upgrade, an unset policy changes behavior immediately to auto review. Operators who require the previous direct-execution behavior must set `FLEETY_POLICY=full_access` or persist that exact value in the Server-owned config.

Rollback is the same explicit setting: set `FLEETY_POLICY=full_access`, restart the server, and later revert the code change if needed.

## Open Questions

None.ARTIFACT_EOF