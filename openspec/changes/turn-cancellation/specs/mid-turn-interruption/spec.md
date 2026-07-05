## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: An explicit CancelTurn frame cancels the in-flight turn

The wire protocol SHALL provide a `CancelTurn` client frame (with an optional conversation id) that cancels the connection's in-flight turn without submitting a new message and without going through triage. Under the full-access policy, the server's mid-turn read loop SHALL react to `CancelTurn` by setting the cancellation flag with an "explicit" reason and immediately emitting an acknowledgement message so the user gets instant feedback; the cancelled turn's closing reply SHALL state that it was cancelled at the user's request and that completed work is preserved (distinct from the wording used when a triaged interjection interrupts the run). A `CancelTurn` received while no turn is in flight SHALL be ignored silently (a cancel racing a just-finished turn must not produce a stray message). Under the require-approval policy the server does not read frames mid-turn, so `CancelTurn` during a gated turn SHALL have no effect; this limitation SHALL be documented in the environment reference.

#### Scenario: explicit cancel acknowledges and closes with cancelled wording

- **WHEN** a turn is running under full access and the client sends `CancelTurn`
- **THEN** the server immediately emits an acknowledgement, the run stops at the next checkpoint, and the closing reply states the turn was cancelled at the user's request with completed work preserved

#### Scenario: idle cancel is silent

- **WHEN** the client sends `CancelTurn` while no turn is in flight
- **THEN** the server emits nothing for it and the connection continues normally

### Requirement: The TUI offers a cancel gesture

While a turn is in flight (message sent, final reply not yet received), pressing Esc in the TUI SHALL send `CancelTurn` and show a cancelling indicator in the status line instead of quitting. Esc SHALL keep its existing meanings with this precedence: a pending approval prompt (Esc = deny) first, then an in-flight turn (Esc = cancel), then quit when idle. Ctrl+C SHALL always quit.

#### Scenario: Esc during a turn cancels instead of quitting

- **WHEN** the user presses Esc while the TUI is waiting for or streaming a reply
- **THEN** the TUI sends `CancelTurn`, shows a cancelling status, and stays open; the subsequent closing reply renders in the conversation pane

#### Scenario: Esc when idle still quits

- **WHEN** the user presses Esc with no turn in flight and no pending approval
- **THEN** the TUI quits as before
