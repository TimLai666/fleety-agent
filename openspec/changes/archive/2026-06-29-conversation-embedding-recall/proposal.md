## Why

`conversation_semantic_search` advertises semantic recall but **falls back to
keyword matching** today (conversation_recall.rs degrades to `keyword_search` and
says so in the result). Meanwhile the wiki already has a working, **local**
embedding stack — EmbeddingGemma (fastembed/ONNX, CPU, no network, downloaded
once, ungated by default) over sqlite-vec — proven end to end in `wiki_embed.rs`.
The pieces to make conversation recall genuinely semantic are all present
(per-user conversation storage, `RecallHit.score: Option<f32>` already there);
what's missing is a per-user conversation vector index that reuses the wiki's
embedding layer.

## What Changes

- **Reuse the wiki's local embedding layer** for conversations: factor the shared
  model access (load-once fastembed model + embed/query helpers + prefixes) so
  both the wiki index and a new conversation index use one model, no duplication.
- **A per-user conversation vector index** (sqlite-vec), one embedding per message,
  stored under the user's own directory so it inherits the privacy boundary.
- **Upgrade `conversation_semantic_search`** to embed the query, run a KNN search
  over the acting user's index, and return hits ranked by cosine similarity
  (`RecallHit.score` populated), newest-first on ties. It **falls back to keyword**
  when embeddings are disabled (`FLEETY_WIKI_EMBED=0`) or the index is empty, so it
  never regresses below today's behavior.
- **Incremental, off-turn indexing**: a conversation's new messages are embedded
  and added to its user's index after the turn (fire-and-forget, never blocking
  the user), with a lazy backfill on first search if the index is missing.

## Non-Goals

- Not changing the keyword tools (`conversation_search`, `conversation_list`)
  — they stay as the always-available fallback.
- Not richer chunking (one message = one chunk in v1; multi-message windows or
  distilled summaries are later refinements).
- Not a new embedding model or a network embedding provider — reuse the local
  EmbeddingGemma stack and its `FLEETY_WIKI_EMBED` switch.
- No cross-user search and no protocol change.

## Capabilities

### New Capabilities

- `conversation-embedding-recall`: a per-user conversation vector index (sqlite-vec)
  built with the wiki's local embedding model, powering real semantic
  conversation search (cosine-ranked, newest-first ties), updated incrementally
  off the turn, with a safe keyword fallback and the existing per-acting-user
  privacy scope.

### Modified Capabilities

- `conversation-recall`: the semantic conversation-search tool becomes
  embedding-ranked (was keyword fallback), still scoped to the acting user and
  still degrading to keyword when embeddings are unavailable.

## Impact

- Affected specs: new `conversation-embedding-recall`; modified `conversation-recall`.
- Affected code:
  - New: crates/fleety-server/src/conversation_embed.rs (per-user conversation index: build/update/search over sqlite-vec)
  - Modified: crates/fleety-server/src/wiki_embed.rs (extract the shared model accessor + embed/query helpers; behavior unchanged), crates/fleety-server/src/conversation_recall.rs (semantic search uses the index, keyword fallback retained), crates/fleety-server/src/conn.rs (off-turn incremental index update for the acting user), crates/fleety-server/src/storage.rs (per-user conversation index path helper), docs/env.md (semantic recall is now real; gating + privacy)
  - Removed: none
