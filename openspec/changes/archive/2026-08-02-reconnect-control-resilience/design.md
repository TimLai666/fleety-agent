## Context

The Daemon already uses a durable JSONL reconnect journal, terminal receipts, success proofs, owner generations, control-version checks, and process-start identity. The remaining gaps are at the request-lifecycle and timing boundaries: the caller waits five seconds while the whole owner-requested candidate sweep has only 4.5 seconds, and a timed-out request has no operator-visible status, cancellation, supersession, retention policy, or owner-aware stale-control recovery. Settlement and quarantine paths can also keep retrying under a lease or make shutdown depend on an unbounded write succeeding.

This change tightens the existing control contract. It does not replace the journal or weaken authentication. It makes a reconnect request observable and recoverable while keeping ordinary Daemon service available when durable control housekeeping encounters a transient filesystem failure.

## Goals / Non-Goals

**Goals:**

- Define a nonce-addressed lifecycle for pending and terminal reconnect requests.
- Expose status, safe cancellation, explicit supersession, retention, and owner-aware stale-control recovery.
- Bound journal append, receipt/proof publication, quarantine, cleanup, and shutdown settlement retries.
- Keep ordinary Daemon service running when a control housekeeping operation is temporarily unavailable, while preserving a fail-closed result for the affected request.
- Couple the CLI wait and whole candidate-sweep budget so the caller contract covers the complete reconnect attempt.
- Preserve authenticated Server identity, secure-channel proof, control-version, process-start, owner-generation, and no-overwrite boundaries.

**Non-Goals:**

- Do not replace the JSONL journal with a database or introduce a second reconnect protocol.
- Do not weaken profile ownership, endpoint authentication, or secure-channel requirements.
- Do not make transport attempts exactly-once.
- Do not add a provider-specific overlay integration.
- Do not redo receipt recovery and control identity protections that already exist.

## Decisions

### The reconnect nonce owns one observable lifecycle

Each submitted nonce SHALL have one active state and one terminal result. The lifecycle SHALL cover submitted, claimed, in-progress, settled, cancelled, superseded, and expired states as applicable. Status lookup SHALL be read-only and SHALL return the current state, profile, owner identity, timestamps, and terminal result when retained.

A terminal receipt SHALL remain authoritative for its retention period. A new request SHALL NOT overwrite an active or retained terminal request with the same nonce. Repeated status reads SHALL be idempotent and SHALL not extend retention.

### Cancellation and supersession require the current owner

Cancellation SHALL be accepted only from the control owner that created or currently owns the request, and only before a terminal authenticated success is durably proved. Supersession SHALL record the replacement nonce and settle the old request as superseded before the replacement can take ownership. A stale or foreign owner SHALL receive an explicit refusal and SHALL not delete, rewrite, or reap another owner's journal or receipt.

The existing authenticated success proof remains authoritative. Cancellation SHALL never erase a durable success proof or turn a successful reconnect into a failure.

### Retention and stale recovery are explicit

Terminal receipts SHALL have a documented retention duration and SHALL be reaped only after that duration, under the same owner and control-version checks used for normal mutations. Stale-control inspection SHALL report the recorded process identity, process-start identity, owner generation, and age. Cleanup SHALL require proof that the owner is no longer live or an explicit operator confirmation with a safe recovery command; it SHALL never blindly remove a live or successor lock.

### Durable housekeeping is bounded and retryable

Journal append, receipt/proof publication, quarantine, directory synchronization, cleanup, and shutdown settlement SHALL use bounded retry policies. A temporary failure SHALL leave the durable evidence intact and produce an actionable error state. The Daemon SHALL continue serving ordinary work when it can maintain its authenticated session, while a pending reconnect request remains visible as unsettled or failed-to-persist rather than disappearing.

A cleanup failure SHALL not hold a global reconnect lease forever. A failed quarantine SHALL not remove an ambiguous success proof or overwrite a different receipt. Repeated attempts SHALL be safe after a process restart.

### The caller and sweep share one budget contract

The CLI's wait for an owner-requested reconnect and the Daemon's whole candidate sweep SHALL derive from one documented budget. The sweep budget SHALL fit inside the caller timeout with reserved margin for durable settlement and response delivery. Candidate shares SHALL be computed from the remaining whole-sweep deadline, and a slow or silent candidate SHALL not consume time reserved for later candidates or settlement.

The ordinary non-reconnect connection path retains its longer endpoint budget. This change affects only owner-requested reconnects and the caller's matching wait.

### Parallel surfaces stay aligned

The shared reconnect state machine SHALL be used by the Daemon owner command, CLI notification path, service lifecycle helpers, TUI guidance, smoke tests, and documentation. Any new nonce operation SHALL have matching parsing, authorization, persistence, output, and error guidance in the CLI and Daemon surfaces.

## Implementation Contract

### Behavior

- Add a nonce status model and lifecycle operations to the existing journal and receipt paths.
- Add owner-aware cancel, supersede, retention, stale inspection, and safe stale recovery operations.
- Route all journal mutations through bounded retry helpers with deterministic terminal errors.
- Ensure ordinary service work is not blocked indefinitely by reconnect cleanup or settlement.
- Use one shared timing contract for the caller timeout, sweep deadline, candidate shares, and settlement margin.
- Keep existing authentication and success-proof checks unchanged except where required to expose lifecycle state.

### Interface and data shape

- Journal events SHALL include enough data to reconstruct lifecycle state, owner, replacement relationship, and timestamps.
- Terminal receipts SHALL identify their state and retention expiry without changing the meaning of an accepted success proof.
- CLI operations SHALL address a request by nonce and SHALL expose stable success/error classes for human and machine output.
- Stale-control inspection SHALL be read-only; cleanup SHALL require owner/process evidence and SHALL report whether anything was removed.
- Timing constants SHALL be defined once or derived from one base budget so caller and Daemon cannot silently diverge.

### Failures and safety

- A malformed complete record, a conflicting record, or an ambiguous state SHALL fail closed and remain available for diagnosis. An incomplete final JSONL append MAY be treated as a recoverable torn tail only when no complete conflicting event follows; the reader SHALL preserve all preceding durable events and SHALL not invent a terminal result.
- A write timeout SHALL not be reported as a successful cancellation, supersession, settlement, or cleanup.
- A foreign owner, live process, mismatched process-start identity, or successor lock SHALL block destructive recovery.
- Retrying an operation after interruption SHALL converge to the existing durable state rather than publish a conflicting result.
- A terminal success proof SHALL be preserved until the retention policy permits reaping.

### Acceptance criteria

- Status is repeatable and returns active, terminal, superseded, cancelled, expired, and ambiguous states with nonce and owner context.
- Cancel and supersede enforce owner and success-proof boundaries, including concurrent callers.
- Retention reaps only eligible terminal records and does not affect active requests.
- Stale inspection and recovery distinguish dead owners from live or reused processes.
- Filesystem faults are bounded, retryable, restart-safe, and do not stop ordinary Daemon service.
- The caller wait exceeds the complete reconnect sweep plus settlement margin, and tests prove slow/silent candidates do not erase later candidate time.
- Existing receipt-recovery and control-identity regressions remain green.

### In scope

The existing Daemon reconnect journal, receipts, proofs, lease/recovery helpers, reconnect command path, CLI notification and guidance, service lifecycle helpers, smoke/unit tests, and reconnect documentation.

### Out of scope

A new storage backend, remote control protocol, transport exactly-once guarantee, provider-specific networking, and unrelated profile or authentication redesign.

## Risks / Trade-offs

More lifecycle states make the journal and CLI richer but increase compatibility and test surface. The state model must remain append-only and reject transitions it cannot prove.

A bounded retry can leave durable housekeeping unfinished when the filesystem remains unavailable. That is preferable to an infinite lease or a Daemon that stops ordinary service; the retained record and status operation provide recovery visibility.

Extending the caller timeout improves slow overlay roaming but makes an interactive command wait longer before reporting failure. The shared budget and explicit status operation preserve a way to inspect or cancel a request instead of forcing repeated blind retries.

Owner-aware stale recovery reduces accidental deletion but requires operators to inspect process identity and explicitly confirm dead ownership. The extra step is necessary because PID reuse and successor locks are real failure modes.

## Migration Plan

1. Add lifecycle events and readers that accept the current journal records without changing existing success-proof meaning.
2. Add the caller/sweep budget contract and bounded housekeeping helpers behind the existing reconnect path.
3. Add CLI status/cancel/supersede/retention and stale-control commands with machine-readable outcomes.
4. Exercise restart, concurrent owner, filesystem-fault, slow-candidate, and retention scenarios in smoke/unit tests.
5. Document retention, recovery, and the matching CLI/Daemon timeout behavior.

## Open Questions

- Should status and cancellation live under `fleetyd reconnect` only, or should `fleety` also proxy them when it can reach the Daemon owner?
- What retention duration best balances post-failure diagnosis with control-directory growth?
- Should supersession be automatic for a newer explicit request, or require an operator flag every time?
