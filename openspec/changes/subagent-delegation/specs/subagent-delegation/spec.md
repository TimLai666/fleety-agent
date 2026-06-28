## ADDED Requirements

### Requirement: Spawn and fork subagents

The system SHALL provide a `spawn_subagent` tool that runs a nested agent loop and returns the subagent's result to the parent. It SHALL support two modes via `mode`: `spawn` (default) starts the subagent with a fresh context seeded only by the briefing `prompt`, and `fork` starts the subagent with the parent conversation's messages inherited as its initial context. In foreground (`run_in_background` false), the call SHALL return the subagent's final `output`; in background it SHALL return immediately with a `task_id` and a running state.

#### Scenario: spawn starts clean, fork inherits context

- **WHEN** `spawn_subagent` is called with `mode="spawn"`
- **THEN** the subagent's initial messages contain only the system prompt and the briefing, not the parent conversation
- **WHEN** `spawn_subagent` is called with `mode="fork"`
- **THEN** the subagent's initial messages include the parent conversation's messages

### Requirement: Capability inheritance with one-level nesting

A subagent SHALL receive the same tool set as the parent agent MINUS the orchestration tools (`spawn_subagent`, `send_subagent_message`, `stop_subagent`, `subagent_status`). Because a subagent has no orchestration tools, it SHALL NOT be able to spawn further subagents (a one-level nesting cap enforced by tool absence, not a runtime check). A subagent SHALL retain every other tool, including `device_exec` to act on other devices.

#### Scenario: subagent lacks orchestration tools but keeps the rest

- **WHEN** a subagent's tool registry is built
- **THEN** it excludes the orchestration tools and includes `device_exec` and the normal file/browser/computer/mcp/wiki tools

### Requirement: Model tier selection

`spawn_subagent` and `send_subagent_message` SHALL accept `model` with the value `main` (default) or `cheap`, choosing which configured provider the subagent runs on. The tier selection SHALL apply to both `spawn` and `fork` modes, so a fork MAY run on a different tier than its parent.

#### Scenario: fork on the cheap tier

- **WHEN** `spawn_subagent` is called with `mode="fork"` and `model="cheap"`
- **THEN** the subagent inherits the parent context but runs on the cheap-tier provider

### Requirement: Isolation mode

`spawn_subagent` SHALL accept `isolation` with the value `none` (default, shared parent workspace) or `worktree`. With `worktree`, the runtime SHALL create a dedicated git worktree for the subagent before it runs and remove it afterwards when unchanged; if the workspace is not a git repository, the call SHALL fail with an actionable error rather than silently downgrading to `none`.

#### Scenario: worktree isolation requires a git workspace

- **WHEN** `spawn_subagent` is called with `isolation="worktree"` in a non-git workspace
- **THEN** the call fails with an actionable error explaining a git repository is required
