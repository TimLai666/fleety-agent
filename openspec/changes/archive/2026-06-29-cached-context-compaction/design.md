## Context

`agent-core`'s `compact_if_needed` (agent.rs) is the headroom-style rolling
summary: when the assembled context exceeds `compact_threshold_chars`, it LLM-
summarizes the middle `[system .. split]` into one `[Summary of earlier
conversation]` system message and keeps the recent `recent_keep_messages`. It is
in-memory only and recomputed every call — so a long conversation re-summarizes
its whole middle on every turn and every reload. Conversations persist as user/
assistant text (tool blobs are collapsed to the final reply), so the cost is the
repeated LLM summarization of a growing transcript. This change caches that
summary and makes the work incremental, while keeping `agent-core` host-free.

## Goals / Non-Goals

**Goals:**
- Persist a conversation's rolling summary + a sequence watermark; reuse it.
- Incremental compaction: summarize only messages past the watermark, fold into
  the cached summary.
- Keep `agent-core` host-free (cache in → cache out; server persists).
- Always-safe fallback: missing/stale cache → full summarization (today's
  behavior).

**Non-Goals:**
- No change to what compaction produces, the other headroom engines, the
  protocol, or making any lossy artifact the source of truth (the event stream
  stays authoritative).

## Decisions

### A compaction cache = summary text + a sequence watermark

Define `CompactionCache { summary: String, summarized_up_to_seq: u64 }`: the
rolling summary so far and the conversation `seq` of the last message folded into
it. It is a derived optimization of the full transcript, never authoritative.

**Alternative:** cache the whole compacted message vector — rejected (larger,
couples to message identity; summary + watermark is the minimal reusable state).

### `agent-core` compaction takes a cache in and returns it out (host-free)

`compact_if_needed` gains an optional `&mut Option<CompactionCache>` (or a small
in/out struct). Behavior: if over threshold,
- with a usable cache (its watermark ≤ the current middle): summarize only the
  **new** middle messages (those after the watermark, up to the new split) and
  **fold** them into the cached summary (a short LLM call over just the delta,
  e.g. "extend this summary with the following newer messages"); update the
  watermark.
- with no/stale cache: summarize the whole middle (today's path) and seed the
  cache.
The function returns the updated cache so the caller can persist it. `agent-core`
performs **no I/O** — it only computes; persistence is the server's job.

**Alternative:** have `agent-core` read/write storage — rejected (breaks the
host-free invariant: `agent-core` depends on no fleety crate / no host I/O).

### The server persists the cache per conversation; loads before, saves after

`fleety-server` storage stores a conversation's `CompactionCache` (e.g.
`fleet/users/<user>/conversations/<id>.compaction.json`, alongside the
user-primary conversation). `conn` loads it before building the turn's messages,
passes it into the run loop, and saves the returned (updated) cache after the
turn. The daemon/CLI are unaffected.

**Alternative:** a single global cache file — rejected (per-conversation is the
natural key; matches the user-primary layout and rolls over with the conversation).

### Invalidation is conservative and always safe

The cache is considered stale (→ ignored, recompute fully) when: the relevant
config changed (different `recent_keep_messages` / `compact_threshold_chars`
recorded with the cache), the watermark is ahead of the loaded history (e.g. the
conversation was edited/truncated), or it fails to parse. A missing/stale cache
is never an error — it degrades to today's full summarization. So the feature can
only speed things up, never change correctness.

**Alternative:** trust the cache blindly — rejected (config/edit drift would
yield a wrong summary; cheap guards keep it safe).

### Watermark semantics tie to conversation `seq`

The watermark is a conversation `seq` (the monotonic per-conversation sequence
from storage). "New middle" = messages whose seq is greater than the watermark
but still older than the recent-keep tail. Because the conversation is append-
only (rollover starts a fresh id), the watermark is stable and monotonic.

## Implementation Contract

**Behavior:** A long conversation, on reload or a follow-up turn, reuses its
cached rolling summary and summarizes only the messages added since the last
compaction, then persists the updated summary + watermark. The model-facing
context is the same shape as today (system + summary + recent N). A missing,
stale, or config-mismatched cache falls back to a full summarization (today's
behavior) and reseeds. `agent-core` does no I/O. Nothing panics; the event
stream remains the source of truth.

**Interfaces / data shapes:**
- `agent_core::CompactionCache { summary: String, summarized_up_to_seq: u64, recent_keep: usize, threshold: usize }` (the last two record the config the summary was built under, for invalidation).
- `compact_if_needed(..., cache: &mut Option<CompactionCache>)` (or a returned `Option<CompactionCache>`): updates/produces the cache; pure compute, no I/O.
- A pure helper `is_cache_usable(cache, history_max_seq, config) -> bool` (watermark within history + config matches) — unit-testable without an LLM.
- A pure helper to pick the "new middle" slice given the watermark + split — unit-testable.
- Storage: `load_compaction(user/conv) -> Option<CompactionCache>` / `save_compaction(user/conv, &cache)`.
- conn: load before the turn, pass into the loop, save the returned cache after.

**Failure modes:** missing cache → full summarize + seed. Stale (config changed / watermark past end / parse error) → ignore + full summarize + reseed. Storage save fails → log, continue (the in-memory turn is unaffected; next turn recomputes). LLM fold call fails → fall back to a full summarize this turn (or skip compaction, as today on provider error). Never panic; never corrupt the conversation; the event stream is always recoverable.

**Acceptance criteria:**
- Pure unit tests (no LLM): `is_cache_usable` (within/ahead watermark, matching/ mismatched config); the new-middle slice selection given watermark + split; cache seeded when absent.
- Compaction tests with a scripted provider (like the existing compaction tests): first compaction seeds a cache + watermark; a second turn with more messages summarizes only the delta (assert the fold prompt covers only the new middle, not the whole history) and advances the watermark; a config change invalidates and recomputes.
- Storage round-trip test for the compaction cache (save → load equal; missing → None; corrupt → None).
- conn-level: the cache is loaded before and saved after a turn (with an injectable storage / provider).
- agent-core stays host-free (`cargo tree -p agent-core` has no `fleety-*`).
- fmt + clippy --workspace -D warnings + test --workspace green.

**Scope boundaries:**
- In: `CompactionCache` type, incremental + cached `compact_if_needed`, the pure usability/slice helpers, storage persist/load, conn load-before/save-after, invalidation, docs, pure + scripted-provider tests.
- Out: changing what compaction produces, the other headroom engines, protocol changes, making any lossy artifact authoritative, cross-conversation cache sharing.

## Risks / Trade-offs

- [cache drift vs the real transcript] → conservative invalidation (config + watermark guards), always-safe fallback to full summarize; the event stream stays authoritative.
- [a lossy summary persisted] → it's a derived cache, never the source of truth; on any doubt it is recomputed from the full transcript.
- [incremental fold quality vs a fresh full summary] → folding can drift over many increments; mitigate by periodically (e.g. every K folds, or when the summary grows past a bound) doing a full re-summarize to refresh. Recorded as a tunable.
- [host-free invariant] → `agent-core` only computes (cache in/out); all persistence is server-side, preserving `cargo tree -p agent-core` cleanliness.
- [interplay with rollover] → rollover starts a new conversation id with no cache (fresh); the old conversation's cache is irrelevant. No special handling needed.
