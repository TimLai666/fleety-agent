# subagent-framework Specification

## Purpose

TBD - created by archiving change 'subagent-core-extraction'. Update Purpose after archive.

## Requirements

### Requirement: Generic subagent mechanism in the core

The agent-core crate SHALL provide a host-agnostic subagent mechanism — `SubagentState`, `SubagentMode`, `SpawnRequest`, the `SubagentHost` trait, `SubagentManager`, and `register_orchestration` — usable by any embedder. The mechanism SHALL NOT depend on any host-specific crate, preserving agent-core's rule of depending on no Fleety crate.

#### Scenario: core builds without any host crate

- **WHEN** agent-core is compiled on its own
- **THEN** it provides the subagent mechanism and its dependency graph contains no `fleety-*` crate


<!-- @trace
source: subagent-core-extraction
updated: 2026-06-28
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/subagent.rs
-->

---
### Requirement: Host trait abstracts all I/O

`SubagentHost` SHALL define every I/O concern the manager needs as a method the embedder implements: resolve a provider for a tier, build a child tool registry (omitting the orchestration tools), produce the initial messages for a mode, build the approval gate, prepare and clean up an isolated workspace, record audit events, and report a completed background subagent. The manager SHALL obtain all I/O through this trait and contain no host-specific code.

#### Scenario: a mock host drives the manager

- **WHEN** a test implements `SubagentHost` with in-memory stubs and calls the manager
- **THEN** the manager runs a nested agent loop using only the trait methods, with no host crate involved


<!-- @trace
source: subagent-core-extraction
updated: 2026-06-28
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/subagent.rs
-->

---
### Requirement: One-level nesting cap by construction

The orchestration tools (`spawn_subagent`, `send_subagent_message`, `stop_subagent`, `subagent_status`) SHALL be added to a registry only by `register_orchestration`, and only at the top level. A child registry returned by `SubagentHost::child_registry` SHALL omit them, so a subagent cannot spawn further subagents.

#### Scenario: child registry lacks orchestration tools

- **WHEN** the host builds a child registry for a subagent
- **THEN** it contains the normal tools but none of the four orchestration tools, while the top-level registry contains them


<!-- @trace
source: subagent-core-extraction
updated: 2026-06-28
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/subagent.rs
-->

---
### Requirement: Manager owns the lifecycle

`SubagentManager` SHALL own the task registry, lifecycle states (Spawned/Running/Done/Failed/Stopped), the concurrency cap, and the `spawn`/`send`/`stop`/`status` operations. A foreground spawn SHALL await the nested loop and return its output; a background spawn SHALL run on a separate task and report completion via `SubagentHost::on_complete`. A subagent error SHALL become a `Failed` state with the error summary as output, never a panic.

#### Scenario: foreground returns output, background reports on completion

- **WHEN** `spawn` runs in the foreground
- **THEN** it returns the subagent's output and a terminal state
- **WHEN** `spawn` runs in the background
- **THEN** it returns a task id immediately and later calls `on_complete` with the terminal state and output

<!-- @trace
source: subagent-core-extraction
updated: 2026-06-28
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/subagent.rs
-->

---
### Requirement: The host records a subagent under a parent-owned child conversation

The subagent host SHALL persist a subagent run's events under a child conversation
(tagged with the child conversation id), owned by the parent turn's acting user,
rather than as untagged device-audit events, and SHALL record a parent→child link.
The core subagent mechanism, the one-level nesting cap, and the manager lifecycle
are unchanged; only the host's persistence and ownership change.

#### Scenario: events are tagged to the child, not untagged audit

- **WHEN** the host records a subagent run's events
- **THEN** they are written tagged to the child conversation id (not as untagged device-audit entries)

#### Scenario: ownership follows the parent's acting user

- **WHEN** the host persists the subagent's child conversation
- **THEN** its owner is the parent turn's acting user, not the device owner

<!-- @trace
source: subagent-conversation-linkage
updated: 2026-06-29
code:
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/device.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/subagent.rs
  - Cargo.toml
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/agent-workflow/src/lib.rs
-->