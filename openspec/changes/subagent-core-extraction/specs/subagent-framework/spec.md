## ADDED Requirements

### Requirement: Generic subagent mechanism in the core

The agent-core crate SHALL provide a host-agnostic subagent mechanism — `SubagentState`, `SubagentMode`, `SpawnRequest`, the `SubagentHost` trait, `SubagentManager`, and `register_orchestration` — usable by any embedder. The mechanism SHALL NOT depend on any host-specific crate, preserving agent-core's rule of depending on no Fleety crate.

#### Scenario: core builds without any host crate

- **WHEN** agent-core is compiled on its own
- **THEN** it provides the subagent mechanism and its dependency graph contains no `fleety-*` crate

### Requirement: Host trait abstracts all I/O

`SubagentHost` SHALL define every I/O concern the manager needs as a method the embedder implements: resolve a provider for a tier, build a child tool registry (omitting the orchestration tools), produce the initial messages for a mode, build the approval gate, prepare and clean up an isolated workspace, record audit events, and report a completed background subagent. The manager SHALL obtain all I/O through this trait and contain no host-specific code.

#### Scenario: a mock host drives the manager

- **WHEN** a test implements `SubagentHost` with in-memory stubs and calls the manager
- **THEN** the manager runs a nested agent loop using only the trait methods, with no host crate involved

### Requirement: One-level nesting cap by construction

The orchestration tools (`spawn_subagent`, `send_subagent_message`, `stop_subagent`, `subagent_status`) SHALL be added to a registry only by `register_orchestration`, and only at the top level. A child registry returned by `SubagentHost::child_registry` SHALL omit them, so a subagent cannot spawn further subagents.

#### Scenario: child registry lacks orchestration tools

- **WHEN** the host builds a child registry for a subagent
- **THEN** it contains the normal tools but none of the four orchestration tools, while the top-level registry contains them

### Requirement: Manager owns the lifecycle

`SubagentManager` SHALL own the task registry, lifecycle states (Spawned/Running/Done/Failed/Stopped), the concurrency cap, and the `spawn`/`send`/`stop`/`status` operations. A foreground spawn SHALL await the nested loop and return its output; a background spawn SHALL run on a separate task and report completion via `SubagentHost::on_complete`. A subagent error SHALL become a `Failed` state with the error summary as output, never a panic.

#### Scenario: foreground returns output, background reports on completion

- **WHEN** `spawn` runs in the foreground
- **THEN** it returns the subagent's output and a terminal state
- **WHEN** `spawn` runs in the background
- **THEN** it returns a task id immediately and later calls `on_complete` with the terminal state and output
