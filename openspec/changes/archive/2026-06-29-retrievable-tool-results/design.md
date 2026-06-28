## Context

Tool results are compressed before reaching the model: `compress_tool_result`
(agent-core/src/compress.rs) runs `SmartCrusher` then `budget_text`, whose marker
reads "... [truncated N chars; full result retained in the event log]". The full
value is genuinely retained — `Event::ToolResult { id, result }`
(agent-core/src/event.rs) carries the complete `result`, keyed by the tool-call
`id`, and that same id appears on the model-visible tool-result message
(`tool_call_id`). But retrieval is inadequate: the marker has no id, the only
tool (`history_list`) returns the last N audit entries verbatim with no
fetch-by-id, no segmenting, and no return-path budget, and it reads the per-device
audit log without scoping to the acting user (a privacy-isolation violation). The
sole long-term full copy today lives in the device `history.jsonl` (append_history
in conn.rs), which is not user-scoped. This change makes truncation locatable and
adds a bounded, user-scoped retrieval path, reusing the existing event log.

## Goals / Non-Goals

**Goals:**
- Truncation markers carry the result id so the agent can fetch precisely.
- A `fetch_tool_result(id, offset?, limit?)` tool returns the full result in
  bounded, budgeted segments (paging), never re-blowing the context.
- Retrieval is confined to the acting user's accessible conversations; the audit
  listing gets the same scope.
- Keep agent-core host-free (marker formatting only; no I/O in core).

**Non-Goals:**
- No change to the first (truncated) view; no protocol change; not the
  cached-context-compaction work; no new full-result store if the event log
  suffices.

## Decisions

### The truncation marker carries the tool-call id (no new id invented)

`Event::ToolResult` is already keyed by the tool-call `id`, and the model already
sees that id as `tool_call_id` on the truncated message. So compression reuses
that id rather than minting a new one: `budget_text` / `compress_tool_result`
take the id and emit "... [truncated N chars; fetch the full result with
fetch_tool_result id=\"<id>\"]". agent-core stays host-free — this is pure string
formatting; it performs no I/O and resolves nothing.

**Alternative:** mint a content-hash id at compress time — rejected (the tool-call
id already uniquely keys the event and is already in the model's context; a second
id is redundant and would need its own mapping).

**Alternative:** name the server tool inside core's marker is mild coupling, but
acceptable: the string is a model-facing hint, not a code dependency, and keeps
the id and the retrieval instruction together where the truncation happens.

### `fetch_tool_result(id, offset?, limit?)` returns bounded, budgeted segments

A new server tool resolves the full result for `id`, then returns
`result[offset .. offset+limit]` (character window) plus `total_chars` and
`next_offset` (null at end). `limit` defaults to the same `max_tool_result_chars`
budget used elsewhere and is capped to it, so a single fetch can never exceed the
normal tool-result budget — paging is the only way to read a very large result,
by design. Non-JSON/string results are rendered to their canonical string first,
then windowed.

**Alternative:** return the whole result on fetch — rejected (defeats the purpose;
one fetch could re-blow the context the truncation just protected).

### Retrieval source = the event log, scoped to the acting user's conversations

Resolution reads the retained `ToolResult { id, result }` events. To make this
user-safe, logged tool-result events are **tagged with their conversation_id**,
and `fetch_tool_result` resolves an id **only within conversations the acting
user can access** (reusing privacy-isolation's `conversation_access`). An id that
exists but belongs to another user's conversation is treated as not found — a
uniform refusal, never confirming its existence (privacy-isolation's no-leak
rule). If an id matches in more than one accessible conversation, the most recent
in the current conversation wins, else the most recent overall in scope.

**Alternative:** add a separate per-user full-result store — rejected for v1
(reuse the event log; only add conversation tagging + scoped resolution). Noted
as a future option if audit-log scanning proves too slow.

### `history_list` gains the same acting-user filter

The existing audit listing currently returns device-wide entries (including full
tool results) with no user scope — the same privacy hole. It is brought under the
acting-user scope: it returns only entries from conversations the acting user can
access. This is part of closing the retrieval privacy gap, not a separate change.

**Alternative:** leave `history_list` as-is — rejected (it would remain a
cross-user leak that undercuts the whole point of scoping retrieval).

## Implementation Contract

**Behavior:** When a tool result is truncated, the model sees a marker naming the
id to fetch. Calling `fetch_tool_result(id)` returns the full result for that id
in a bounded window (default = the tool-result budget), with `total_chars` and
`next_offset` so the agent can page through a large result without ever exceeding
the budget in one call. Resolution and the audit listing are confined to the
acting user's accessible conversations; an id outside that scope is reported as
not found with no hint that it exists. agent-core does no I/O; nothing panics; the
event log remains the source of truth.

**Interfaces / data shapes:**
- agent-core: `budget_text(text, max_chars, id: Option<&str>)` and
  `compress_tool_result(value, max_chars, id: &str)` — marker includes the id
  when present; behavior unchanged when no id.
- server tool `fetch_tool_result`: params `{ id: string, offset?: integer>=0, limit?: integer>=1 }`; returns `{ id, total_chars, offset, returned_chars, next_offset: integer|null, content }`; risk = Read.
- storage: resolve a tool result by id within a user's accessible conversations,
  e.g. `tool_result_for(user, conversation_hint, id) -> Option<(value, conversation_id)>`; logged tool-result events tagged with conversation_id.
- conn: register `fetch_tool_result` bound to the acting user (like
  conversation_recall); tag tool-result events with the conversation_id when
  logging; pass the tool-call id into compression.

**Failure modes:** unknown/out-of-scope id → "not found" (uniform; never reveals
existence). offset past end → empty content, `next_offset=null`, correct
`total_chars`. limit above the budget → clamped to the budget. corrupt/unreadable
event line → skipped; if nothing resolves → not found. Storage read error →
errors-as-message, no panic. Missing id arg → validation error.

**Acceptance criteria:**
- agent-core unit tests: marker includes the id when given; `compress_tool_result`
  with an id round-trips through the marker; no-id path unchanged (existing tests
  still green).
- `fetch_tool_result` tests (injectable storage): returns the right window;
  `next_offset` pages to the end then null; `limit` clamped to the budget; offset
  past end is empty + null; unknown id → not found.
- Privacy tests: acting user A cannot fetch an id from user B's conversation (not
  found, no existence hint); `history_list` returns only the acting user's
  entries. (Extends privacy-isolation's suite.)
- agent-core stays host-free (`cargo tree -p agent-core` has no `fleety-*`).
- fmt + clippy --workspace -D warnings + test --workspace green.

**Scope boundaries:**
- In: id-carrying truncation marker (agent-core), `fetch_tool_result` (segmented +
  budgeted + user-scoped), conversation tagging of tool-result events, scoped
  resolution, `history_list` acting-user filter, docs, tests.
- Out: changing the first truncated view, protocol changes, cached-context-
  compaction, a separate full-result store, cross-user sharing.

## Risks / Trade-offs

- [tool-call id collisions across turns/conversations] → resolve within the
  current conversation first, then most-recent in the acting user's scope; ids
  are unique within a conversation, which is the common case.
- [return-path re-bloat] → hard budget + offset/limit paging + reported total;
  one fetch can never exceed the normal tool-result budget.
- [privacy] → user-scoped resolution + uniform not-found; deny cross-user even
  when the id is known (privacy-isolation no-leak). `history_list` brought under
  the same scope so the hole is fully closed.
- [audit-log scan cost for resolution] → acceptable for v1 (bounded by recent
  history); a per-user index is a noted future optimization if needed.
- [host-free invariant] → core only formats the marker; all resolution/I/O is
  server-side, preserving `cargo tree -p agent-core` cleanliness.
- [marker coupling to a server tool name] → it is a model-facing hint string, not
  a code dependency; accepted to keep the id and the how-to-retrieve together.
