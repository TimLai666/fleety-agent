## Why

Fleety currently has a default full-access policy and a human approval policy, but no unattended policy that evaluates whether a proposed tool call serves the user's stated objective. This makes fully autonomous operation either too permissive or dependent on a human approval path, especially for high-risk actions.

## What Changes

- Add an `auto_review` execution policy that sends every non-read tool call, including critical actions, to the configured cheap model before execution.
- Provide the reviewer with the user's objective, relevant bounded conversation context, tool name, arguments, risk class, and deterministic danger signals.
- Convert dangerous-command and sensitive-path detection from an unconditional pre-execution refusal into trusted warnings supplied to the reviewer when `auto_review` is active.
- Require strict structured reviewer decisions: approve or deny with a bounded reason; reviewer failure, timeout, invalid output, or unavailable provider denies execution.
- Prevent the reviewer from calling tools or changing the candidate action.
- Record the review decision, model identity, risk, danger signals, and outcome in the audit trail without persisting secrets.

## Non-Goals (optional)

- Do not make `auto_review` fail open when the reviewer is unavailable.
- Do not add a new model-provider implementation; reuse the existing cheap provider tier.
- Do not remove operating-system permissions, filesystem errors, or transport authentication.
- Do not require interactive human approval in `auto_review`, including for critical actions.

## Capabilities

### New Capabilities

- `auto-review`: Unattended cheap-model review and execution gating for read, mutate, and critical tool operations, including trusted danger warnings, strict failure behavior, and audit records.

### Modified Capabilities

- `runtime-configuration`: Add `auto_review` as a valid tool policy and define its cheap-provider and failure behavior.
- `agent-conduct-policy`: Define auto review as the third execution posture and require all non-read actions to pass its review.
- `command-execution`: Preserve deterministic dangerous-command detection as review input instead of an unconditional refusal under `auto_review`.
- `filesystem-tools`: Allow the auto-review path to evaluate sensitive-path mutations using trusted danger signals while preserving normal path resolution and OS-level failures.
- `audit-history`: Record auto-review decisions and denial reasons without exposing secrets.

## Impact

- Affected specs: `auto-review`, `runtime-configuration`, `agent-conduct-policy`, `command-execution`, `filesystem-tools`, `audit-history`
- Affected code:
  - Modified: `crates/agent-core/src/approval.rs`
  - Modified: `crates/agent-core/src/agent.rs`
  - Modified: `crates/fleety-server/src/conn.rs`
  - Modified: `crates/fleety-server/src/main.rs`
  - Modified: `crates/fleety-server/src/providers.rs`
  - Modified: `crates/fleety-tools/src/lib.rs`
  - Modified: `crates/fleety-tools/src/terminal.rs`
  - Modified: `docs/env.md`
  - Modified: `docs/tools.md`
  - Modified: `README.md`
  - Modified: `crates/fleety-server/tests/server_smoke.rs`
  - Modified: `crates/fleety-daemon/tests/fleetyd_smoke.rs`
  - Modified: `crates/fleety-cli/tests/cli_smoke.rs`
