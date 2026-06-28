## Context

Conversations persist as `fleet/devices/<device>/conversations/<id>.jsonl`, one `{seq, message}` per line (storage.rs append/load). `seq` gives order but there is no timestamp; only the audit `history.jsonl` carries `ts_secs`. The only semantic search in the system is the wiki (wiki_embed.rs: EmbeddingGemma via fastembed + a sqlite-vec store under `wiki/.index/`). No tool lets the agent search its own past conversations. This change adds time to conversation events and gives the agent keyword + semantic recall over them, reusing the wiki's embedding/sqlite-vec layer rather than inventing a second one.

## Goals / Non-Goals

**Goals:**
- Record wall-clock time on conversation events (additive, backward compatible).
- Let the agent keyword- and semantic-search the device's past conversations; results carry conversation_id, seq, ts, role, snippet, and are ordered so precedence/time are clear.
- Reuse the wiki embedding + sqlite-vec infrastructure; keyword fallback when embeddings are off; lazy backfill for old conversations.
- Per-device scope; best-effort, non-blocking indexing.

**Non-Goals:**
- No change to resume/replay or conversation load.
- No rollover/distillation (that is conversation-lifecycle).
- No cross-device recall in v1.
- No new embedding model or store; reuse the wiki's.

## Decisions

### Add `ts_secs` to conversation events, additively

The conversation record becomes `{seq, ts_secs, message}`. `ts_secs` is written on append (wall clock at write time) and read back via serde with a default of 0 so existing `{seq, message}` lines still parse (same `flatten`/default trick already used for `history.jsonl`'s `ts_secs`). Resume/replay ignore `ts_secs`; it is purely additive metadata for recall and display.

**Alternative:** derive time from file mtime or the audit log — rejected (mtime is per-file not per-message; joining to the audit log is fragile). Storing it inline is simplest and local.

### Recall is keyword + semantic, reusing the wiki embedding/sqlite-vec layer

Two retrieval paths, both per-device:
- **Keyword** (`conversation_search`): scan the device's conversation JSONL files for a substring/词 match. Always available (no model needed). Returns matches newest-first with seq+ts.
- **Semantic** (`conversation_semantic_search`): embed the query with the same EmbeddingGemma model the wiki uses and KNN against a per-device conversation vector index (a sqlite-vec store, sibling to the wiki index, e.g. under `fleet/devices/<device>/.recall/`). Returns top-k with a relevance score plus seq+ts. When `FLEETY_WIKI_EMBED=0` (no model), it degrades to the keyword path with a note.
- A `conversation_list` returns the device's conversations with first/last seq and last-activity ts, so the agent can orient in time.

Sharing the wiki's embedding code (factor the embed + sqlite-vec calls so both the wiki and recall use them) avoids a second model/store and keeps behavior consistent.

**Alternative:** a separate embedding stack for conversations — rejected (duplicate model download + store; drift).

### Incremental best-effort indexing with lazy backfill

When a message is appended in a turn, after it is persisted the server indexes it into the device's recall index (spawned, best-effort, gated by `FLEETY_WIKI_EMBED`) — the same posture as the wiki's per-note async reindex. A conversation that predates the index (or any gap) is backfilled lazily: the first semantic search for a device that finds the index missing/stale enqueues a background backfill of that device's conversations, while that search falls back to keyword so it still returns something. Indexing never blocks a turn and never fails it.

**Alternative:** index synchronously on append — rejected (adds latency to every turn). Index only at rollover — rejected (recall wouldn't work within a live long conversation).

### Per-device scope and result shape

Recall operates over the conversations of the device the agent is acting for (path-scoped to `fleet/devices/<device>/conversations/`). Every result carries `{conversation_id, seq, ts_secs, role, snippet}`; keyword results sort newest-first, semantic results sort by score but include ts so the agent can reason about time. This matches the per-device memory model (conversations already live under the device).

**Alternative:** global recall across all devices — rejected for v1 (privacy/scope; a device's agent reasoning over another device's chats is a bigger decision).

## Implementation Contract

**Behavior:** New conversation events are stored with a timestamp; old ones still load (ts unknown). The agent can call `conversation_search` (keyword) and `conversation_semantic_search` (semantic) over its device's past conversations and get back matches with conversation id, sequence, time, role, and a snippet, ordered so it knows what came before what; `conversation_list` shows the device's conversations with last-activity time. Indexing happens in the background and never blocks or fails a turn; with embeddings disabled, semantic search degrades to keyword. Nothing panics.

**Interfaces / data shapes:**
- Conversation record: `{ seq: u64, ts_secs: u64 (serde default 0), message: Message }`.
- `RecallHit { conversation_id: String, seq: u64, ts_secs: u64, role: String, snippet: String, score: Option<f32> }`.
- Tools: `conversation_search { query, limit? }`, `conversation_semantic_search { query, limit? }`, `conversation_list { limit? }` — registered for the agent (server tools).
- Shared embedding layer: the embed-query + sqlite-vec upsert/query calls used by the wiki are factored so the recall index reuses them; recall index path `fleet/devices/<device>/.recall/recall.db`.
- Storage: an iterator over a device's conversation events (for keyword scan + backfill), and the append path writes `ts_secs`.

**Failure modes:** embeddings unavailable / model missing → semantic search falls back to keyword with a note; index missing/stale → lazy backfill enqueued, keyword used meanwhile. Corrupt/oversized line in a JSONL → skipped, scan continues. Index write failure → logged, recall still serves keyword. Unknown/blank query → empty result with a clear message, no error. Never block or fail a turn; never panic.

**Acceptance criteria:**
- Storage test: append writes `{seq, ts_secs, message}`; an old `{seq, message}` line loads with `ts_secs == 0`; resume/replay still return events in order.
- Keyword recall test (no model needed): seeded conversations → `conversation_search` returns the expected hits with seq+ts, newest-first.
- Result-shape test: `RecallHit` carries conversation_id/seq/ts/role/snippet; `conversation_list` reports last-activity ts.
- Degradation test: with embeddings disabled, `conversation_semantic_search` returns keyword results plus the degradation note (no error).
- Semantic path: the embed+sqlite-vec reuse compiles and a unit test exercises the shared layer with a tiny in-memory/temp index (model-dependent KNN over real text marked manual-verify).
- Content review: docs/env.md notes conversation indexing reuses FLEETY_WIKI_EMBED + the model dir.
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.

**Scope boundaries:**
- In: `ts_secs` on conversation events; `conversation_search` / `conversation_semantic_search` / `conversation_list` tools; per-device recall index reusing the wiki embedding/sqlite-vec layer; incremental async indexing + lazy backfill + keyword fallback; docs; storage/keyword/shape/degradation tests.
- Out: rollover/distillation, cross-device recall, resume/replay changes, a new embedding model/store, changes to the wiki's own behavior.

## Risks / Trade-offs

- [embedding every message is costly] → async best-effort, gated by FLEETY_WIKI_EMBED, lazy backfill; keyword always works without it.
- [old conversations have no ts] → additive default 0; recall reports time as unknown for those, order still via seq.
- [sharing wiki embedding code risks coupling] → factor the shared calls cleanly; wiki behavior unchanged and still tested.
- [per-device scope may feel limiting] → deliberate for v1 (privacy/scope); cross-device recall is a later, explicit decision.
- [index/JSONL drift] → lazy backfill reconciles; keyword scan is the always-correct fallback.

## Implementation note (v1 scope)

v1 ships **keyword recall** (`conversation_search`) + `conversation_list`, fully
per-user (conversations are user-primary after privacy-isolation), plus the
`ts_secs` timestamp on every event. `conversation_semantic_search` **degrades to
keyword with a note** in this build; the embedding-ranked per-user vector index
(reusing the wiki's fastembed + sqlite-vec layer) and the incremental background
indexing in `conn` are a **documented follow-up** — they are not required for the
core value ("the agent can search its own past conversations, with time order"),
which keyword recall + listing already deliver. The "indexing never blocks a
turn" guarantee holds trivially in v1 (there is no indexing step), and semantic
search never errors (it returns keyword matches). When the vector index lands,
`conversation_semantic_search` upgrades in place behind the same `FLEETY_WIKI_EMBED`
gate.
