## ADDED Requirements

### Requirement: Run a model-written workflow script

The system SHALL provide a `run_workflow` tool that takes a JavaScript `script` and executes it to completion on an embedded engine, returning the script's result. The script body runs inside an async wrapper, so it MAY use top-level `await` and SHALL `return` its result; it orchestrates work with injected globals. The engine and bindings SHALL live in a dedicated `agent-workflow` crate so the agent framework core does not depend on the engine.

#### Scenario: a workflow script runs and returns its result

- **WHEN** `run_workflow` is given a script that awaits some `agent(...)` calls and returns a value
- **THEN** the script runs to completion and the tool returns that value

### Requirement: agent() runs a leaf subagent

The injected `agent(opts)` global SHALL run exactly one foreground subagent through the shared subagent manager and resolve to that subagent's output. A subagent launched from a workflow SHALL be a leaf: its tool registry contains neither the orchestration tools nor `run_workflow`, so it cannot spawn subagents or nested workflows (the one-level cap holds).

#### Scenario: workflow agents cannot nest

- **WHEN** a workflow's `agent()` runs a subagent
- **THEN** that subagent's registry excludes `spawn_subagent` and `run_workflow`

### Requirement: Deterministic control-flow primitives

The runtime SHALL inject `parallel(thunks)` (run thunks concurrently, await all), `pipeline(items, ...stages)` (run each item through the stages), `phase(name)` (mark a phase), and `log(msg)` (record progress), in addition to the engine's native `Promise.all`. These let a script express sequential, parallel, and pipelined orchestration.

#### Scenario: parallel runs agents concurrently

- **WHEN** a script calls `parallel([() => agent(a), () => agent(b)])`
- **THEN** both subagents run concurrently and the call resolves to both outputs

### Requirement: Never-panic failure handling

A missing or empty `script`, a parse error, a script that throws uncaught, or a failed `agent()` step SHALL surface as an actionable error from `run_workflow`; the embedded engine SHALL NOT panic the server process. A `agent()` step failure SHALL reject that step's promise so the script MAY catch it.

#### Scenario: an uncaught script error is reported, not fatal

- **WHEN** a workflow script throws an error that it does not catch
- **THEN** `run_workflow` returns an actionable error containing the message and the server keeps running

### Requirement: The framework core stays engine-free

The agent-core crate SHALL depend on neither the JavaScript engine nor any host crate. The engine dependency SHALL be confined to the `agent-workflow` crate.

#### Scenario: core has no engine dependency

- **WHEN** agent-core is built on its own
- **THEN** its dependency graph contains neither `boa_engine` nor any `fleety-*` crate
