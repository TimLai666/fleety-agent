## Context

The subagent host (subagent.rs) builds the subagent's messages, runs a turn, and
in `record_events` writes each event with `append_history(device_id, ev)` — the
device audit log, **untagged** (no conversation id, no owner). On completion it
seeds a 4000-char summary into the **parent** conversation (`on_complete`, with
`context` = the parent conversation). The subagent uses
`acting_for_device(device_id)` for its system prompt/owner, not the parent's
acting user. So a subagent's full record is neither a retrievable conversation nor
linked to its parent, and its events are untagged (unreachable by
`fetch_tool_result` and outside the user privacy boundary). This change records a
subagent run as a child conversation owned by the parent's acting user, tags its
events, and links it from the parent — reusing the existing conversation storage,
owner index, and tagged-audit machinery.

## Goals / Non-Goals

**Goals:** a subagent run is a retrievable, user-owned child conversation;
its events are conversation-tagged (fetchable + privacy-scoped); the parent
conversation links to the child with a parent→children index.

**Non-Goals:** changing how subagents run (mechanism, nesting cap, lifecycle);
removing the parent's summary seed; promoting child conversations as standalone
top-level threads.

## Decisions

### A subagent run is a child conversation, id derived from its task id

Each subagent run gets a child conversation id derived from its task id (a stable
`sub-<task_id>` form). Its transcript (the user prompt it was given and its
assistant output, plus any intermediate messages persisted the same way normal
conversations are) is stored under that id, so recall and listing can find it.

**Alternative:** keep only untagged audit events — rejected (not retrievable, not
attributable; defeats the point).

**Alternative:** invent a brand-new id space — rejected (the task id already
uniquely names the run; derive from it so parent links and audit correlate).

### The child conversation is owned by the parent's acting user

The subagent's child conversation owner is the **parent turn's acting user**, not
`acting_for_device`. This requires threading the parent's acting user into the
subagent host (it currently reads the device owner). Ownership makes the child
user-scoped under privacy-isolation, consistent with the parent.

**Alternative:** own it by the device — rejected (a subagent acts on behalf of the
user who spawned it; device ownership would misattribute and could leak across
users on a shared device).

### Events are tagged to the child conversation

`record_events` writes with `append_history_tagged(device_id, child_id, ev)`
instead of untagged `append_history`, so the subagent's tool results are reachable
by `fetch_tool_result` and bounded by the user privacy scope — the same guarantees
as a normal turn.

**Alternative:** leave untagged — rejected (unfetchable, unscoped; inconsistent
with retrievable-tool-results).

### The parent records an explicit link to the child

The parent conversation links to the child at two points: the spawn (the
`spawn_subagent` result carries the child conversation id) and the completion (the
seed already references the run; it now names the child id). The server keeps a
parent→children index (conversation id → child conversation ids) so a conversation
can enumerate the subagents it spawned and open each one's full record. The
parent's own summary seed stays as the inline synthesis.

**Alternative:** rely on grepping the audit log by task id — rejected (no
first-class navigation; brittle string correlation).

### Reuse existing storage machinery

Child conversations use the same per-user conversation storage and owner index as
normal conversations (`register_conversation_owner`, conversation paths), and the
same `append_history_tagged`. The only new persistence is the small parent→children
link index.

**Alternative:** a separate subagent store — rejected (duplicates conversation
storage; would need its own recall/fetch/privacy wiring).

## Implementation Contract

**Behavior:** When a parent turn (acting as user U) spawns a subagent, the subagent
runs as a child conversation owned by U, identified by a child id derived from the
task id. Its transcript is retrievable (recall/listing) and its events are tagged
to the child id (so its tool output is fetchable via `fetch_tool_result` and scoped
to U). The parent conversation carries a link to the child id at the spawn and at
completion, and the server can enumerate a conversation's child subagents. The
parent still receives the result summary seed. Nothing panics; persistence
failures are logged and don't abort the subagent or the parent turn.

**Interfaces / data shapes:**
- A child conversation id derived from the task id (`sub-<task_id>` or equivalent).
- subagent host: `record_events` tags events with the child id; the run registers
  the child conversation's owner = the parent's acting user; the host is given the
  parent's acting user (threaded in, replacing `acting_for_device` for ownership).
- storage: a parent→children link (e.g. `subagent_link(parent_id, child_id)` +
  `subagent_children(parent_id) -> Vec<String>`), persisted like the other small
  json indexes under `fleet/`.
- parent linkage: the `spawn_subagent` tool result includes `child_conversation_id`;
  the completion seed names it.
- conn: thread the parent's acting user into the subagent host construction.

**Failure modes:** child owner registration / link write fails → logged, the
subagent still runs and the parent still gets its summary (best-effort record, like
today). Missing acting user (guest parent) → child is unowned (same as a guest's
own conversation today), not attributed to any user. Duplicate task id → idempotent
child id; link index dedups. Nesting cap unchanged (subagents can't spawn deeper).

**Acceptance criteria:**
- Unit: child conversation id derivation from a task id is stable/deterministic.
- Storage: parent→children link round-trips (add child, list children, dedup);
  child conversation owner is the provided user; child events are tagged with the
  child id (verifiable via the existing audit/`tool_result_for` path).
- The subagent's events are written with the child id (not untagged) — assert via a
  fake host/storage that `append_history_tagged` is used with the child id.
- A spawned subagent's result/spawn carries the child conversation id.
- fmt + clippy --workspace -D warnings + tests green; agent-core stays host-free
  (this is all server-side; the core subagent mechanism is unchanged).
- The end-to-end live spawn + recall of a child conversation is manual-verify.

**Scope boundaries:**
- In: child conversation id + ownership (parent's acting user) + tagged events +
  parent→children link/index + spawn/result linkage + docs + the testable pieces.
- Out: changing the subagent run mechanism/nesting/lifecycle, removing the summary
  seed, promoting children as standalone top-level threads, agent-core changes.

## Risks / Trade-offs

- [recall noise from many child conversations] → children are linked from the
  parent, not promoted as standalone threads; listing visibility is tunable; recall
  still benefits from having the full record available.
- [ownership threading] → the host must receive the parent's acting user; a guest
  parent yields an unowned child (consistent with guest conversations today).
- [storage growth] → child conversations reuse normal conversation storage; bounded
  by subagent usage; prunable with the parent on rollover/GC (future).
- [host-free invariant] → all changes are server-side; the core subagent mechanism
  and Host trait shape are unchanged, so `agent-core` stays clean.
- [interaction with retrievable-tool-results / recall] → tagging child events and
  owning the child conversation is exactly what those features need, so this closes
  the gap rather than adding a parallel path.
