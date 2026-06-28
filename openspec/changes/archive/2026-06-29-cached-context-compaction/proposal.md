## Why

Context compaction (the headroom-style rolling summary in `agent-core`) currently
runs **in memory only** and is **recomputed from scratch every turn and every
reload**: a long conversation re-summarizes its entire middle on every reconnect
and on each subsequent turn, paying a full LLM summarization pass each time. The
work is correct but wasteful — the summary of the old middle barely changes
between turns. Caching the rolling summary (with a sequence watermark) makes
compaction incremental: reuse the cached summary, summarize only the new middle
beyond the watermark.

## What Changes

- **Persist the rolling summary** for a conversation plus a `summarized_up_to_seq`
  watermark (the last message folded into the summary).
- **Incremental compaction**: when the context is over threshold and a cached
  summary exists, summarize only the messages between the watermark and the new
  split, fold them into the cached summary, and persist the updated summary +
  watermark — instead of re-summarizing the whole middle.
- **Host-free boundary kept**: `agent-core`'s compaction takes an optional
  cached summary + watermark as input and returns the updated pair; it never
  touches storage. The server loads the cache before a turn and persists it
  after (the daemon/CLI front-ends are unaffected).
- **Invalidation**: the cache is recomputed when the relevant config changes
  (recent-keep / threshold) or when the conversation is rolled over / edited; a
  missing or stale cache simply falls back to a full summarization (today's
  behavior), so it is always safe.

## Non-Goals

- Not changing what compaction produces (still a lossy rolling summary + recent
  N + system); only making it incremental + cached.
- Not the other headroom engines (SmartCrusher / CodeCompressor / CacheAligner)
  — those are unchanged.
- Not persisting anything lossy as the source of truth: the full event stream
  remains authoritative; the cached summary is a derived optimization.
- No protocol change.

## Capabilities

### New Capabilities

- `cached-context-compaction`: a persisted rolling-summary cache (summary +
  sequence watermark) that makes context compaction incremental — reuse the
  cached summary and summarize only new messages past the watermark, instead of
  re-summarizing the whole middle on every turn/reload; host-free core (cache in
  → cache out), server-side persistence, safe fallback to full summarization.

### Modified Capabilities

(none in spec terms — compaction is internal to agent-core; this adds the
caching layer around it.)

## Impact

- Affected specs: new `cached-context-compaction`.
- Affected code:
  - Modified: crates/agent-core/src/agent.rs (compaction accepts an optional
    cached summary + watermark and returns the updated pair; incremental
    summarization of only the new middle), crates/fleety-server/src/storage.rs
    (persist/load a conversation's compaction cache), crates/fleety-server/src/conn.rs
    (load the cache before a turn, save it after), docs/env.md (note any threshold/cache env)
  - New: none required (a small cache type can live in agent-core or storage)
  - Removed: none
