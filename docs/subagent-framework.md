# Subagent framework (agent-core)

Subagent delegation is a **generic capability of `agent-core`**, not a Fleety
feature bolted onto the server. Any embedder of agent-core gets subagents by
implementing one trait. agent-core itself stays dependency-free (it depends on
no `fleety-*` crate), so the mechanism travels with the framework when it is
extracted to its own repository.

Lives in [`crates/agent-core/src/subagent.rs`](../crates/agent-core/src/subagent.rs).
Fleety's wiring is [`crates/fleety-server/src/subagent.rs`](../crates/fleety-server/src/subagent.rs).

## The model

A **subagent** is a nested [`run_turn`] loop with its own messages, a chosen
model tier, and a tool registry equal to the parent's **minus** the
orchestration tools. Because a subagent has no orchestration tools, it cannot
spawn further subagents — a **one-level nesting cap enforced by construction**,
not a runtime check.

```
parent registry  = host tools  +  register_orchestration(manager)
child  registry  = host tools                                   (no orchestration → no nesting)
```

## The split

| In agent-core (generic) | Supplied by the embedder (`SubagentHost`) |
|---|---|
| `SubagentState` / `SubagentMode` / `SpawnRequest` | which provider a tier maps to |
| `SubagentManager`: task registry, state machine, concurrency cap | how to build the child tool registry |
| `spawn` / `send` / `stop` / `status` + the `run_turn` loop | system prompt + conversation history |
| the four orchestration tools + `register_orchestration` | isolated workspaces (prepare / cleanup) |
| gate selection (full-access vs require-approval) | audit (record events) and "report back" |

The manager owns the **what** (lifecycle, concurrency, orchestration); the host
owns the **where/how** (all I/O). The manager contains no host-specific code.

## The host trait

```rust
#[async_trait]
pub trait SubagentHost: Send + Sync + 'static {
    fn resolve_provider(&self, tier: &str) -> Arc<dyn ModelProvider>;
    async fn capture_context(&self) -> String;                 // opaque token (e.g. conversation id)
    async fn child_registry(&self, workspace: Option<&str>) -> ToolRegistry;   // NO orchestration tools
    async fn initial_messages(&self, mode: SubagentMode, context: &str, prompt: &str) -> Vec<Message>;
    async fn prepare_workspace(&self, isolation: &str, task_id: &str) -> Result<Option<String>>;
    async fn cleanup_workspace(&self, workspace: Option<&str>) -> bool;        // false = kept (had changes)
    async fn record_events(&self, events: &[Event]);
    async fn on_complete(&self, task_id: String, context: String, state: SubagentState, output: String);
}
```

- **`tier` and `isolation` are opaque strings.** The core never hardcodes
  `"main"`/`"cheap"` or git worktrees — the host interprets them. So a different
  embedder can offer different tiers or a different isolation strategy.
- **`capture_context` → `on_complete`** threads an opaque token (Fleety uses the
  conversation id) so a background subagent reports back to the right place even
  after the foreground moved on.
- **`cleanup_workspace` returns whether it removed the workspace.** A host that
  keeps a dirty isolated workspace returns `false`; the manager then appends a
  note to the output so the work is not lost.

## Wiring it

```rust
let host = Arc::new(MyHost { /* ... */ });
let manager = SubagentManager::new(host.clone() as Arc<dyn SubagentHost>, policy, max_concurrent);
// IMPORTANT: register the orchestration tools ONLY on a top-level registry.
agent_core::register_orchestration(&mut top_level_tools, manager);
```

The four tools (`spawn_subagent`, `send_subagent_message`, `stop_subagent`,
`subagent_status`) drive the manager. See [tools.md](tools.md#subagents-delegation)
for their agent-facing contract.

## Behaviour

- **`spawn` / `fork`** — `spawn` starts a fresh context seeded by the briefing;
  `fork` inherits the conversation (`host.initial_messages` decides what that
  means). Either runs on any tier.
- **Foreground vs background** — foreground awaits and returns the output;
  background returns a `task_id` immediately and reports completion through
  `host.on_complete`, which a host typically uses to proactively resume a
  coordinator turn (Fleety does this via `conn::drive_turn`).
- **Concurrency cap** — a background spawn past `max_concurrent` is refused, not
  queued. Floor of 1.
- **Never panics** — a subagent error becomes a `Failed` state whose output is
  the error summary.

## Fleety's host

[`FleetyHost`](../crates/fleety-server/src/subagent.rs) implements the trait
with: providers via `ProviderTiers` (`FLEETY_CHEAP_MODEL_*`), child registry via
`build_full_registry`, messages/audit via `Storage`, git worktrees for
`isolation="worktree"`, and a proactive wake turn via `conn::drive_turn` held
under a per-connection turn lock. `FLEETY_SUBAGENT_MAX_CONCURRENT` sets the cap.
