## Why

The reconnect journal now records durable ownership and terminal results, but the remaining contract is not yet safe for slow overlay links, interrupted cleanup, or an operator who needs to recover a timed-out request. A reconnect caller waits five seconds while the Daemon has only a 4.5-second candidate sweep, and lifecycle records have no explicit status, cancellation, supersession, retention, or owner-aware stale-lock operation. These gaps can turn a legitimate roaming failure into an opaque timeout or leave recovery dependent on retrying commands in a particular order.

## What Changes

- Define a nonce-addressed reconnect lifecycle with inspect/status, safe cancel or supersede, terminal-result retention, and owner-aware stale-control recovery.
- Make journal append, receipt/proof publication, quarantine, cleanup, and settlement failures bounded, retryable, and fail-closed without stopping the Daemon's ordinary service loop or overwriting another request's result.
- Couple the caller wait and the whole candidate sweep budget so cross-network endpoints receive a fair bounded attempt and a request cannot outlive the caller's contract silently.
- Preserve the existing control-version, process-start identity, authenticated candidate, owner-generation, and durable-success-proof boundaries.
- Add CLI/Daemon regressions for slow candidates, silent peers, torn writes, interrupted cleanup, stale ownership, repeated status, cancellation, supersession, retention expiry, and concurrent callers.

## Non-Goals

- Do not replace the existing JSONL journal with a database or introduce a second reconnect protocol.
- Do not weaken authenticated Server identity, profile owner validation, or secure-channel requirements to make a candidate connect faster.
- Do not integrate Tailscale or any other specific overlay provider.
- Do not make reconnect transport attempts exactly-once; only the request lifecycle and terminal settlement are durable.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `connection-profiles`: define reconnect request lifecycle, terminal-result retention, cancellation, supersession, and owner recovery.
- `daemon-resilience`: define bounded cross-endpoint reconnect budgets and failure recovery that keeps ordinary service available.
- `service-lifecycle`: define reconnect-control ownership, stale-lock inspection, and safe cleanup behavior.
- `cli-command-surface`: expose the operator-facing reconnect lifecycle operations and their failure guidance.

## Impact

- Affected specs: `connection-profiles`, `daemon-resilience`, `service-lifecycle`, `cli-command-surface`
- Affected code:
  - Modified: `crates/fleety-daemon/src/main.rs`
  - Modified: `crates/fleety-tools/src/service.rs`
  - Modified: `crates/fleety-cli/src/server.rs`
  - Modified: `crates/fleety-cli/src/main.rs`
  - Modified: `crates/fleety-cli/src/config_panel.rs`
  - Modified: `crates/fleety-daemon/tests/fleetyd_smoke.rs`
  - Modified: `crates/fleety-cli/tests/cli_smoke.rs`
  - Modified: `docs/design-cli-config.md`
  - Modified: `docs/roadmap.md`
  - Modified: `README.md`
