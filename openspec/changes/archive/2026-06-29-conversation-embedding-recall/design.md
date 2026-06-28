## Context

`conversation_semantic_search` (conversation_recall.rs) calls `keyword_search`
and labels the result as a degraded fallback. The wiki proves the real path:
`wiki_embed.rs` loads EmbeddingGemma 300M (Q8) via fastembed (local ONNX, CPU,
downloaded once, ungated unless `FLEETY_WIKI_EMBED=0`), embeds text with
`embed_texts`/`with_model` using DOC/QUERY prefixes, stores vectors in a
sqlite-vec `vec0` table inside a single `wiki.db`, and KNN-searches with
`embedding MATCH ?` ordered by cosine distance. Conversations are stored per user
under `fleet/users/<user>/conversations/*.jsonl` (enumerated by
`user_conversation_ids`, loaded by `load_user_conversation`), each event carrying
`seq` + `ts_secs`; `RecallHit` already has a `score: Option<f32>`. This change
gives conversations their own per-user vector index, reusing the wiki's embedding
layer, and makes `conversation_semantic_search` real.

## Goals / Non-Goals

**Goals:**
- One shared local embedding model for wiki + conversations (no duplicate loads).
- A per-user conversation vector index (sqlite-vec), one embedding per message.
- `conversation_semantic_search` returns cosine-ranked hits, newest-first on ties,
  scoped to the acting user, with a keyword fallback when embeddings are
  unavailable.
- Indexing is incremental and off the turn — never adds user-visible latency.

**Non-Goals:**
- No change to keyword tools, no richer chunking, no new/network embedding model,
  no cross-user search, no protocol change.

## Decisions

### Reuse the wiki's embedding layer via a small shared accessor

Factor the model access out of `wiki_embed.rs` into a shared embedding helper (the
process-global load-once fastembed model, `embed_texts`/query helpers, DOC/QUERY
prefixes, the `FLEETY_WIKI_EMBED` gate, the model cache dir). `wiki_embed` keeps
its index logic but calls the shared accessor; the new conversation index uses the
same accessor. One model in memory, one gate, no drift.

**Alternative:** duplicate the model loading in a conversation module — rejected
(two ~300MB model loads, divergent prefixes/gating).

### A per-user conversation index, one embedding per message

Store a sqlite-vec database per user at
`fleet/users/<user>/conversations/.index/conversations.db` (mirrors the wiki's
single-db design, but under the user's directory so it inherits the privacy
boundary). Schema: `meta(model, dim)`, `chunks(key, conversation_id, seq, ts_secs,
role, snippet)`, and a `vec0` virtual table `vec_chunks(key, embedding)`. One
message = one chunk (messages are already discrete units); embed the message
content with the DOC prefix.

**Alternative:** a single global index partitioned by user — rejected (per-user
file matches the conversation layout and the privacy boundary; deletion/rollover
stay local to the user).

**Alternative:** multi-message windows / distilled summaries as chunks — deferred
(per-message is the simplest correct unit; richer chunking is a later refinement).

### Separate the index storage/query layer from the model

Split the module into (a) a storage/query layer that inserts precomputed vectors
and runs KNN (testable with synthetic vectors, no model) and (b) the embedding
step that turns text into vectors (the model). This makes the index logic
unit-testable deterministically without loading the 300MB model.

**Alternative:** couple them — rejected (untestable without the model, slow/flaky).

### Search: embed the query, KNN, map to RecallHit, keyword fallback

`conversation_semantic_search` embeds the query (QUERY prefix), runs KNN over the
acting user's index, maps the hit keys back to `chunks` rows → `RecallHit` with
the cosine similarity in `score`, ordered by score then `ts_secs` (newest-first on
ties). When embeddings are disabled (`FLEETY_WIKI_EMBED=0`) or the index is empty
/ unbuildable, it falls back to `keyword_search` (today's behavior) — so it never
regresses. A Guest (no acting user) returns nothing, unchanged.

**Alternative:** drop the keyword fallback — rejected (semantic must degrade
gracefully when the model is off or the index is cold).

### Incremental, off-turn indexing + lazy backfill

After a turn completes, the acting user's new messages are embedded and inserted
into their index, fire-and-forget (like the wiki's write-time reindex), so the
turn is never blocked. On a search, if the user's index is missing or behind, a
bounded backfill builds it from `load_user_conversation` first. Index build/update
failures are non-fatal — search falls back to keyword.

**Alternative:** index synchronously in the turn — rejected (adds latency to the
user's turn). **Alternative:** only batch-build at boot — rejected (new messages
wouldn't be searchable until restart).

## Implementation Contract

**Behavior:** With embeddings enabled, `conversation_semantic_search(query)`
returns the acting user's most semantically similar past messages, cosine-ranked,
newest-first on ties, each `RecallHit` carrying `score`, `conversation_id`, `seq`,
`ts_secs`, `role`, `snippet`. New messages become searchable after their turn
(off-turn indexing). With embeddings disabled or the index unavailable, it returns
the keyword result (today's behavior) without error. A Guest gets nothing. Indexes
are per-user; one user's search never reads another's index. agent-core is
untouched (this is all server-side); nothing panics; failures degrade to keyword.

**Interfaces / data shapes:**
- Shared embedding accessor (in/near `wiki_embed.rs`): `enabled() -> bool`,
  `with_model(cache_dir, f)` / `embed_texts(...)` / query-embed, DOC/QUERY prefixes
  — reused by both indexes.
- `conversation_embed.rs`: a per-user index type with
  `index_path(home, user) -> PathBuf`, an `upsert(conversation_id, &[(seq, ts, role, text)])`
  that embeds + writes, and `search(query_vec, limit) -> Vec<(key/meta, score)>`;
  plus a pure storage/query layer accepting precomputed vectors.
- storage: `conversation_index_path(user)` helper.
- `conversation_recall.rs`: `ConversationSemanticSearch::call` uses the index, maps
  to `RecallHit` (score = cosine), falls back to `keyword_search`.
- conn: an off-turn hook that updates the acting user's index with the turn's new
  messages (fire-and-forget).

**Failure modes:** embeddings disabled → keyword. Model download/load fails →
keyword (logged). Index open/build/query fails → keyword (logged), never panics.
Empty query → keyword or empty (consistent with keyword tool). Index behind →
lazy backfill; if backfill fails → keyword. Concurrent updates → single-writer per
user index (serialize writes), reads tolerate a slightly stale index.

**Acceptance criteria:**
- Pure storage/query tests (synthetic vectors, no model): inserting vectors and
  KNN returns the nearest in cosine order; meta(dim) round-trips; an empty index
  yields no hits.
- Mapping test: KNN keys → `RecallHit` carry score + correct conversation_id/seq/
  ts/role/snippet; ordering is score then newest-first.
- Fallback tests: with `FLEETY_WIKI_EMBED=0` or an empty/unavailable index,
  `conversation_semantic_search` returns the keyword result and does not error;
  Guest returns nothing.
- The wiki's existing semantic-search tests still pass after the accessor refactor.
- Off-turn indexing is non-blocking and failures don't fail the turn (verified via
  the hook returning immediately / errors swallowed).
- fmt + clippy --workspace -D warnings green; full test suite green.
- The end-to-end real-embedding ranking quality is environment-dependent
  (downloads the model) and is manual-verify.

**Scope boundaries:**
- In: shared embedding accessor refactor, per-user conversation index
  (build/update/search), semantic-search upgrade with keyword fallback, off-turn
  indexing + lazy backfill, per-user index path, docs, tests.
- Out: keyword tools, richer chunking, new/network embedding model, cross-user
  search, protocol changes, agent-core changes.

## Risks / Trade-offs

- [model is heavy / may be absent] → reuse the existing gated, download-once local
  model; everything degrades to keyword when it's off or unavailable.
- [indexing latency] → off-turn fire-and-forget + lazy backfill; the turn never
  waits on embedding.
- [index staleness vs the transcript] → conversations are append-only; the index
  only grows; a behind index triggers a bounded backfill; reads tolerate slight
  lag. The transcript (JSONL) stays the source of truth.
- [privacy] → per-user index path + the tool is already acting-user scoped; Guest
  gets nothing; no cross-user index reads.
- [refactor risk to the wiki] → the accessor extraction must keep wiki behavior
  identical; guarded by the wiki's existing tests.
- [testability without the model] → the storage/query layer is split out and
  tested with synthetic vectors; only the (manual-verify) embedding step needs the
  model.
- [disk growth] → one small sqlite db per user; vectors are int8/f32 of a single
  modest model; acceptable, and prunable with the conversation on rollover/delete
  (future).
