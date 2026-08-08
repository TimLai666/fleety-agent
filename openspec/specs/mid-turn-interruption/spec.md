# mid-turn-interruption Specification

## Purpose

TBD - created by archiving change 'mid-turn-interruption'. Update Purpose after archive.

## Requirements

### Requirement: New messages are read while a turn is running

The server SHALL read incoming client messages concurrently with an in-progress turn rather than only after it completes. The turn SHALL run as a background task while the connection loop awaits both the turn's completion and the next inbound message; a single in-flight turn SHALL still be serialized so two turns never run at once.

#### Scenario: a message arriving mid-turn is observed immediately

- **WHEN** a turn is running and the user sends another message
- **THEN** the server reads that message before the turn completes (rather than it waiting in the transport)

#### Scenario: an approval gate never discards an early follow-up

- **GIVEN** `require_approval` is active and a turn is waiting for an approval decision
- **WHEN** one or more user messages arrive before the matching decision
- **THEN** the gate retains those messages in arrival order and the connection processes them after the gated turn instead of discarding them


<!-- @trace
source: mid-turn-interruption
updated: 2026-06-29
code:
  - docs/env.md
  - crates/agent-workflow/src/lib.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/triage.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/gemini.rs
  - crates/fleety-server/src/main.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/effort.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-core/src/model.rs
-->

---
### Requirement: A running turn can be cancelled at safe checkpoints

The goal loop (`drive_to_goal`) SHALL accept a lightweight cancellation flag and check it between goal iterations. In addition, the core turn loop (`run_turn_streaming_cached`) SHALL accept an optional cancellation flag and check it at two finer checkpoints inside a turn: before each tool call is executed, and before each model call. When cancellation is observed at any checkpoint, the run SHALL stop cleanly and return what it completed so far (not an error): tool calls from the current model response that have not started executing SHALL NOT run and SHALL each be fed back as a sentinel tool result whose text contains `cancelled by user before execution` (so every ToolCall has a matching ToolResult and the journal, compaction, and recovery paths stay consistent), and the turn outcome SHALL be marked cancelled. It SHALL NOT interrupt a tool that is already executing, nor a stream that is already emitting. A never-cancelled (or absent) flag SHALL leave behavior identical to today, and the `run_turn` / `run_turn_streaming` wrappers SHALL keep their existing signatures by passing no flag.

#### Scenario: cancel between goal iterations stops further work

- **WHEN** the cancellation flag is set while a turn is between goal iterations
- **THEN** the turn does not begin another iteration and ends cleanly, returning the work done so far

#### Scenario: cancel between tool calls skips the remaining calls with sentinel results

- **WHEN** a model response requests multiple tool calls and the cancellation flag is set after the first call finishes but before the second starts
- **THEN** the second and later calls do not execute, each is recorded with a sentinel tool result containing `cancelled by user before execution`, no further model call is made, and the turn outcome is marked cancelled

##### Example: two tool calls, cancelled after the first

- **GIVEN** a scripted model response requesting tool calls T1 then T2
- **WHEN** the flag is set while T1 is executing
- **THEN** T1's real result is recorded, T2 gets the sentinel result, and the provider receives exactly one model call

#### Scenario: a mid-execution tool is not interrupted

- **WHEN** the cancellation flag is set while a tool call is already running
- **THEN** that tool runs to completion and cancellation takes effect at the next checkpoint (before the next tool / model call / iteration)

#### Scenario: never cancelled preserves current behavior

- **WHEN** a turn runs to completion with a flag that is never set
- **THEN** its behavior is identical to before this change

---
### Requirement: A cancelled run's work is preserved for the next turn

Because cancellation stops the run *between* turns (never mid-turn), every completed turn is persisted to the conversation history as usual, so a follow-up turn that runs the new message reconstructs the prior work into its context. Recovery SHALL NOT be a machine-level resume; the agent decides, from the prior work in context, whether to continue it or pivot.

#### Scenario: prior work appears in the next turn

- **WHEN** a run is cancelled after completing some turns and the new message then runs as a follow-up turn on the same conversation
- **THEN** the follow-up turn's context includes the completed prior work, and the agent may continue or redirect


<!-- @trace
source: mid-turn-interruption
updated: 2026-06-29
code:
  - docs/env.md
  - crates/agent-workflow/src/lib.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/triage.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/gemini.rs
  - crates/fleety-server/src/main.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/effort.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-core/src/model.rs
-->

---
### Requirement: A triage decides how to handle a mid-turn message

When a new message arrives during a turn, the server SHALL classify it with one lightweight model call, given the new message and a compact summary of the active turn, producing one of: `interrupt_now`, `queue_after`, or `ignore`. The decision-text parsing SHALL be a pure function. A triage failure or unparseable output SHALL default to `queue_after` (do not interrupt in-progress work). Every user message SHALL carry a non-empty, bounded client-generated message id. The server SHALL echo that id in every structured acknowledgement (`interrupting`, `queued`, `ignored`, or `rejected`) so a client retires only the matching optimistic message, including when acknowledgements arrive out of order or are duplicated. Because this is a breaking frame-shape change, clients and servers SHALL require exact equality with `PROTOCOL_VERSION` before authentication, identity learning, credential publication, device registration, or other session state is committed. Pending interjections SHALL be FIFO and bounded by both count and total payload size; duplicate or invalid ids and overflow SHALL be rejected immediately before triage, attachment conversion, cancellation, or queue mutation. (Routing triage to the cheap tier specifically is a follow-up.)

#### Scenario: interrupt_now cancels and starts a new turn

- **WHEN** triage returns `interrupt_now`
- **THEN** the cancellation flag is set, the current turn stops at its next checkpoint, and the new message is run as the next turn

#### Scenario: queue_after waits for the current turn

- **WHEN** triage returns `queue_after`
- **THEN** the current turn finishes and the new message is processed afterward, and the user receives an acknowledgement that it was queued

#### Scenario: triage failure is conservative

- **WHEN** the triage call fails or its output cannot be parsed
- **THEN** the message is treated as `queue_after` (the in-progress turn is not interrupted)

#### Scenario: ignored message does not leave a client turn pending

- **WHEN** triage returns `ignore`
- **THEN** the server acknowledges it with the `ignored` disposition and the client retires only that optimistic message without ending the active turn

#### Scenario: a full interjection queue rejects new work

- **WHEN** accepting another interjection would exceed the count or payload-size bound
- **THEN** the server acknowledges it with the `rejected` disposition, does not retain it, and tells the user to resend after the active work completes

#### Scenario: acknowledgements identify the exact optimistic message

- **WHEN** acknowledgements for multiple mid-turn messages arrive out of order, are duplicated, or name an unknown id
- **THEN** the client applies each acknowledgement only to the matching message id and leaves every other optimistic message unchanged

#### Scenario: incompatible protocol is rejected before session state changes

- **WHEN** either peer advertises a protocol version other than the exact current `PROTOCOL_VERSION`
- **THEN** the connection is rejected before authentication, identity learning, credential publication, or device registration

#### Scenario: overflow does not invoke triage

- **WHEN** an interjection would exceed the count or byte bound
- **THEN** the server rejects it before reading the active-turn summary, calling the triage provider, converting attachments, requesting cancellation, or changing the queue

#### Scenario: failed-turn cleanup revokes queued acknowledgements safely

- **WHEN** a provider turn fails and its journal cannot be closed
- **THEN** the server rejects every accepted-but-not-started queued message by its exact id, emits one `turn_cleanup_failed` error that includes both failures, emits no `Done`, and closes the connection so the user can reconnect and resend

##### Example: triage decisions

| triage output | action |
|---|---|
| interrupt_now | cancel current turn, run new message next |
| queue_after | finish current turn, then run new message |
| ignore | do not run; acknowledge only |
| (failed / unparseable) | queue_after (default) |

<!-- @trace
source: mid-turn-interruption
updated: 2026-06-29
code:
  - docs/env.md
  - crates/agent-workflow/src/lib.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/triage.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/agent-core/src/openai.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/gemini.rs
  - crates/fleety-server/src/main.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/effort.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/voice.rs
  - crates/agent-core/src/model.rs
-->

---
### Requirement: An explicit CancelTurn frame cancels the in-flight turn

The wire protocol SHALL provide a `CancelTurn` client frame (with an optional conversation id) that cancels the connection's in-flight turn without submitting a new message and without going through triage. Under the full-access policy, the server's mid-turn read loop SHALL react to `CancelTurn` by setting the cancellation flag with an "explicit" reason and immediately emitting an acknowledgement message so the user gets instant feedback; the cancelled turn's closing reply SHALL state that it was cancelled at the user's request and that completed work is preserved (distinct from the wording used when a triaged interjection interrupts the run). A `CancelTurn` received while no turn is in flight SHALL be ignored silently (a cancel racing a just-finished turn must not produce a stray message). Under the require-approval policy the server does not read frames mid-turn, so `CancelTurn` during a gated turn SHALL have no effect; this limitation SHALL be documented in the environment reference.

#### Scenario: explicit cancel acknowledges and closes with cancelled wording

- **WHEN** a turn is running under full access and the client sends `CancelTurn`
- **THEN** the server immediately emits an acknowledgement, the run stops at the next checkpoint, and the closing reply states the turn was cancelled at the user's request with completed work preserved

#### Scenario: idle cancel is silent

- **WHEN** the client sends `CancelTurn` while no turn is in flight
- **THEN** the server emits nothing for it and the connection continues normally

---
### Requirement: The TUI offers a cancel gesture

While a turn is in flight (message sent, final reply not yet received), pressing Esc in the TUI SHALL send `CancelTurn` and show a cancelling indicator in the status line instead of quitting. Esc SHALL keep its existing meanings with this precedence: a pending approval prompt (Esc = deny) first, then an in-flight turn (Esc = cancel), then quit when idle. Ctrl+C SHALL always quit.

#### Scenario: Esc during a turn cancels instead of quitting

- **WHEN** the user presses Esc while the TUI is waiting for or streaming a reply
- **THEN** the TUI sends `CancelTurn`, shows a cancelling status, and stays open; the subsequent closing reply renders in the conversation pane

#### Scenario: Esc when idle still quits

- **WHEN** the user presses Esc with no turn in flight and no pending approval
- **THEN** the TUI quits as before
