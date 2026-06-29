## ADDED Requirements

### Requirement: New messages are read while a turn is running

The server SHALL read incoming client messages concurrently with an in-progress turn rather than only after it completes. The turn SHALL run as a background task while the connection loop awaits both the turn's completion and the next inbound message; a single in-flight turn SHALL still be serialized so two turns never run at once.

#### Scenario: a message arriving mid-turn is observed immediately

- **WHEN** a turn is running and the user sends another message
- **THEN** the server reads that message before the turn completes (rather than it waiting in the transport)

### Requirement: A running turn can be cancelled at safe checkpoints

The goal loop (`drive_to_goal`) SHALL accept a lightweight cancellation flag and check it between goal iterations. When cancellation is observed at that checkpoint, the run SHALL stop cleanly without starting another iteration and return what it completed so far (not an error). It SHALL NOT interrupt a tool that is already executing, a stream that is already emitting, nor the current turn mid-flight. A never-cancelled flag SHALL leave behavior identical to today. (A finer per-tool-call checkpoint is a follow-up — see the design's open questions.)

#### Scenario: cancel between goal iterations stops further work

- **WHEN** the cancellation flag is set while a turn is between goal iterations
- **THEN** the turn does not begin another iteration and ends cleanly, returning the work done so far

#### Scenario: a mid-execution tool is not interrupted

- **WHEN** the cancellation flag is set while a tool call is already running
- **THEN** that tool runs to completion and cancellation takes effect at the next checkpoint (before the next tool / iteration)

#### Scenario: never cancelled preserves current behavior

- **WHEN** a turn runs to completion with a flag that is never set
- **THEN** its behavior is identical to before this change

### Requirement: A cancelled run's work is preserved for the next turn

Because cancellation stops the run *between* turns (never mid-turn), every completed turn is persisted to the conversation history as usual, so a follow-up turn that runs the new message reconstructs the prior work into its context. Recovery SHALL NOT be a machine-level resume; the agent decides, from the prior work in context, whether to continue it or pivot.

#### Scenario: prior work appears in the next turn

- **WHEN** a run is cancelled after completing some turns and the new message then runs as a follow-up turn on the same conversation
- **THEN** the follow-up turn's context includes the completed prior work, and the agent may continue or redirect

### Requirement: A triage decides how to handle a mid-turn message

When a new message arrives during a turn, the server SHALL classify it with one lightweight model call, given the new message and a compact summary of the active turn, producing one of: `interrupt_now`, `queue_after`, or `ignore`. The decision-text parsing SHALL be a pure function. A triage failure or unparseable output SHALL default to `queue_after` (do not interrupt in-progress work). (Routing triage to the cheap tier specifically is a follow-up.)

#### Scenario: interrupt_now cancels and starts a new turn

- **WHEN** triage returns `interrupt_now`
- **THEN** the cancellation flag is set, the current turn stops at its next checkpoint, and the new message is run as the next turn

#### Scenario: queue_after waits for the current turn

- **WHEN** triage returns `queue_after`
- **THEN** the current turn finishes and the new message is processed afterward, and the user receives an acknowledgement that it was queued

#### Scenario: triage failure is conservative

- **WHEN** the triage call fails or its output cannot be parsed
- **THEN** the message is treated as `queue_after` (the in-progress turn is not interrupted)

##### Example: triage decisions

| triage output | action |
|---|---|
| interrupt_now | cancel current turn, run new message next |
| queue_after | finish current turn, then run new message |
| ignore | do not run; acknowledge only |
| (failed / unparseable) | queue_after (default) |
