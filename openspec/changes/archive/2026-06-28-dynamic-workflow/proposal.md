## Why

Subagents fan out independent work and an agent team coordinates dependent workers, but both are model-driven each turn — flaky and hard to reproduce for repeated engineering work. A **dynamic workflow** lets the agent write a short JS script that deterministically orchestrates its own subagents (sequential, parallel, pipelined, phased) — the same idea as Claude Code's Workflow, but internal: `agent()` runs a Fleety subagent, not an external CLI. This turns "summon the right number of agents as the work unfolds, retire the rest" into explicit, version-controllable control flow.

## What Changes

- New crate `agent-workflow` (depends on agent-core + boa_engine), isolating the JS engine so agent-core stays lean and boa-free.
- New tool `run_workflow` (Mutate): the agent passes a JS `script`; it runs to completion and returns the script's result. Mirrors Claude Code's Workflow tool.
- Injected JS globals: `agent(opts)` runs one foreground subagent via the shared `SubagentManager` and returns its output; `parallel(thunks)`, `pipeline(items, ...stages)`, `phase(name)`, `log(msg)`; a `meta` block names the workflow and its phases.
- Execution: boa runs on a dedicated single-thread runtime (boa `Context` is `!Send`); `agent()` awaits the shared `SubagentManager` there; the final result bridges back to the tool call over a channel.
- The boa async bridge (`JS await` of a Rust future, `Promise.all` for parallel) is already validated by a scratch probe on this platform.

## Non-Goals

- **No external coding-agent CLIs** (Codex/Gemini/Cursor — ODW's model). `agent()` only runs Fleety's own subagents.
- No worker↔worker direct channels (the lead/script coordinates; same stance as the agent-team layer).
- Not a declarative data format — the whole point is a dynamic JS script.
- No JS engine other than boa.
- No nesting escape: a subagent launched by a workflow is a leaf (it has neither the orchestration nor the workflow tool), so the one-level cap holds.
- Not changing the existing subagent/agent-team tools.

## Capabilities

### New Capabilities

- `dynamic-workflow`: a `run_workflow` tool that executes a model-written JS workflow script on an embedded boa engine, binding `agent()`/`parallel()`/`pipeline()`/`phase()`/`log()` to the shared subagent manager, isolated in a new `agent-workflow` crate, with deterministic control flow and actionable (never-panic) failure handling.

### Modified Capabilities

(none)

## Impact

- Affected specs: new capability dynamic-workflow. Modified: none.
- Affected code:
  - New: crates/agent-workflow/ (new crate: the boa runtime, JS global bindings, the run_workflow tool), added to the workspace members in the root Cargo.toml
  - Modified: crates/fleety-server/src/conn.rs (register run_workflow at the top level with the connection's SubagentManager), docs/tools.md (run_workflow), prompts/protocol.md (when to reach for a workflow)
  - Removed: none
- Key acceptance: agent-core still depends on no boa and no fleety crate; boa is confined to agent-workflow; a workflow script with agent() + Promise.all runs and returns the right result; workspace clippy -D and tests stay green.
