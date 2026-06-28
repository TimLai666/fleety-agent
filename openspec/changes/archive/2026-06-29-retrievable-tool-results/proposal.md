## Why

Tool results are truncated for the model (SmartCrusher + a character budget),
with the marker "full result retained in the event log". That reversibility
claim is only half true today: the full result **is** kept (the event log stores
`ToolResult { id, result }` with the complete value), but the agent has **no
precise, safe way to get it back when it actually needs it**:

- The truncation marker carries **no id**, so the agent cannot ask for "the full
  version of *that* truncated result".
- The only retrieval tool, `history_list`, returns the **last N audit entries
  verbatim** — no fetch-by-id, no segment (offset/limit), and **no budget on the
  return path**, so pulling one big result can dump several large results back
  into context and blow it up again.
- `history_list` reads the device audit log and is **not scoped to the acting
  user**, so on a shared device it can surface another user's tool output —
  violating the privacy-isolation hard rule.

So "reversible" stops at "the bytes still exist somewhere"; it does not yet mean
"the agent can retrieve them precisely, safely, and in bounded chunks".

## What Changes

- **Truncation markers become locatable**: when a tool result is truncated, the
  marker includes the result's id (the tool-call id that already keys the event)
  so the agent knows exactly what to fetch.
- **A `fetch_tool_result(id, offset?, limit?)` tool**: returns the full result
  for that id **in bounded segments**, applying the same character budget on the
  way back and reporting total length + the next offset so the agent can page —
  retrieval can never re-blow the context.
- **Retrieval is user-scoped**: `fetch_tool_result` resolves an id only within
  conversations the acting user can access, and `history_list` gains the same
  acting-user filter — closing the shared-device privacy hole.

## Non-Goals

- Not changing what the model sees first (results are still truncated up front;
  this adds an on-demand way to retrieve the rest).
- Not the cached-context-compaction work — orthogonal (that compresses the
  conversation summary; this retrieves compressed tool output).
- No protocol change — `fetch_tool_result` is an ordinary tool call.
- Not introducing a second full-result store if the existing event log suffices;
  reuse it, adding only the scoping/tagging needed for safe retrieval.

## Capabilities

### New Capabilities

- `retrievable-tool-results`: locatable truncation markers (carrying the result
  id) plus a user-scoped fetch-tool-result tool that returns the full,
  event-log-retained tool output in bounded, budgeted segments; retrieval is
  confined to the acting user's accessible conversations, and the existing audit
  listing is brought under the same privacy scope.

### Modified Capabilities

- `privacy-isolation`: tool-result retrieval (the fetch-tool-result tool) and the
  audit listing (the history-list tool) are constrained to the acting user,
  consistent with the user-as-privacy-boundary rule.

## Impact

- Affected specs: new `retrievable-tool-results`; modified `privacy-isolation`.
- Affected code:
  - Modified: crates/agent-core/src/compress.rs (truncation marker carries the result id; `compress_tool_result`/`budget_text` take the id), crates/agent-core/src/agent.rs (pass the tool-call id into compression), crates/fleety-server/src/tools.rs (new `fetch_tool_result` tool; `history_list` acting-user filter), crates/fleety-server/src/storage.rs (resolve a tool result by id within a user's accessible conversations; tag logged tool results with their conversation), crates/fleety-server/src/conn.rs (register `fetch_tool_result` with the acting user; tag tool-result events with conversation_id), docs/env.md (document retrieval + scoping)
  - New: none required (reuse the event log)
  - Removed: none
