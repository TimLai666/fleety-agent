## Why

The agent cannot look back at its own past conversations. Conversation history is persisted as `{seq, message}` JSONL per device, but there is no tool to search it, no semantic search over it (the only semantic search is the wiki), and the records carry no wall-clock time — only a sequence number. So the agent can't answer "when did we last work on X" or pull a relevant exchange from a prior conversation. This is also the prerequisite for safely rolling conversations over (the next change): old conversations are only safe to leave behind if they remain findable.

## What Changes

- **Timestamps on conversation events**: each stored conversation event gains a `ts_secs` alongside its `seq` (additive, backward compatible — old lines read as unknown/0), so recall can report both order (seq) and actual time.
- **Agent recall tools** over the device's own past conversations: a keyword search and a semantic search (the latter reusing the wiki's embedding + sqlite-vec infrastructure), returning matches with `conversation_id`, `seq`, `ts`, role, and a snippet — ordered so the agent knows time/precedence — plus a list of the device's conversations with last-activity time.
- **Incremental, best-effort indexing**: conversation messages are embedded into a per-device semantic index as they are appended (async, gated by the existing `FLEETY_WIKI_EMBED`); keyword search always works by scanning the JSONL even when embeddings are off; a pre-existing conversation is backfilled lazily on first search.
- Recall is **per-device scoped** (the agent searches the conversations of the device it is acting for), matching Fleety's per-device memory model.

## Non-Goals

- Not changing how conversations are stored/loaded for resume (only adding `ts_secs` and an index alongside).
- Not rolling conversations over or distilling them — that is the dependent `conversation-lifecycle` change.
- Not cross-device recall in v1 (kept per-device; cross-device search can come later).
- Not changing the wiki; only reusing its embedding/sqlite-vec building blocks.

## Capabilities

### New Capabilities

- `conversation-recall`: timestamped conversation events plus agent tools to keyword- and semantic-search the device's past conversations, returning time-and-order-aware results; incremental best-effort embedding (reusing the wiki index infra) with keyword fallback and lazy backfill; per-device scope.

### Modified Capabilities

(none — conversation storage gains an additive timestamp field; resume/replay behavior is unchanged.)

## Impact

- Affected specs: new `conversation-recall`.
- Affected code:
  - Modified: crates/fleety-server/src/storage.rs (write/read `ts_secs` on conversation events; expose iteration for keyword scan + index backfill), crates/fleety-server/src/wiki_embed.rs (reuse the embedding + sqlite-vec layer for a per-device conversation index, or a sibling module that shares it), crates/fleety-server/src/conn.rs (index a message after it is appended, best-effort async), docs/env.md (note conversation indexing reuses FLEETY_WIKI_EMBED / model dir)
  - New: crates/fleety-server/src/conversation_recall.rs (the recall tools `conversation_search` / `conversation_semantic_search` / `conversation_list` and the per-device conversation index built on the shared embedding layer)
  - Removed: none
