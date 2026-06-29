## Why

A subagent's work is barely recorded and not linked to its parent. Today the
subagent host writes its turn events with `append_history(device_id, ev)` — the
device audit log, **untagged** (no conversation, no owner). It is not a stored,
retrievable conversation: `conversation_search`/recall don't see it, the new
embedding index doesn't index it, and (because the events are untagged) the
`fetch_tool_result` retrieval and privacy scoping can't reach them. The only thing
that survives into a real conversation is a **4000-char summary** seeded back to
the parent on completion. And there is no first-class parent→child link: a
subagent has a `task_id` string but no conversation id, so you can't navigate from
"this conversation spawned a subagent" to "here is what that subagent did".

This loses the subagent's actual reasoning, makes its tool output unretrievable
and unattributable, and breaks traceability — and it undercuts the recently added
conversation recall and retrievable-tool-results, which both rely on
conversation-tagged, user-owned records.

## What Changes

- **Each subagent run is a child conversation.** It gets a child conversation id
  (derived from its task id) and its transcript is persisted under that id, owned
  by the **parent's acting user** — so it is retrievable (recall / listing) and
  user-scoped like any conversation.
- **Its events are conversation-tagged.** The subagent's audit events are written
  with the child conversation id (not untagged), so tool results are reachable by
  `fetch_tool_result` and respect the user privacy boundary.
- **The parent links to the child.** The parent conversation records an explicit
  parent→child link (the spawn point and the completion both reference the child
  conversation id), and the server keeps a parent→children index, so you can go
  from a conversation to the subagents it spawned and read each one's full record.

## Non-Goals

- Not changing how subagents *run* (the core mechanism, the one-level nesting cap,
  the manager lifecycle) — only how their record is stored and linked.
- Not removing the parent's result summary seed — it stays; the child conversation
  is the full record behind it.
- Not surfacing subagent child conversations as top-level user conversations in
  normal listings beyond what recall already does (they are linked from the
  parent, not promoted as standalone threads) — exact listing visibility is a
  detail for design.

## Capabilities

### New Capabilities

- `subagent-conversation-linkage`: a subagent run is recorded as a child
  conversation (owned by the parent's acting user, retrievable, with
  conversation-tagged events so its tool output is fetchable and user-scoped), and
  the parent conversation carries an explicit link to that child (spawn point +
  completion), with a parent→children index for traceability.

### Modified Capabilities

- `subagent-framework`: the host persists a subagent's events under a child
  conversation owned by the parent's acting user (was untagged device audit), and
  records the parent→child link.

## Impact

- Affected specs: new `subagent-conversation-linkage`; modified `subagent-framework`.
- Affected code:
  - Modified: crates/fleety-server/src/subagent.rs (record events tagged to a child conversation id; register the child conversation + its owner = the parent's acting user; thread the parent's acting user instead of the device owner), crates/fleety-server/src/storage.rs (parent→children link index; register child conversation owner reuse), crates/fleety-server/src/conn.rs (record the spawn→child link in the parent; pass the parent's acting user into the subagent host), docs/env.md
  - New: none required (reuse conversation storage + owner index + tagged audit)
  - Removed: none
