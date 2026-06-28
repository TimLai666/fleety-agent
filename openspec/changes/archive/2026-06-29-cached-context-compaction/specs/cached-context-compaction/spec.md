## ADDED Requirements

### Requirement: Context compaction reuses a cached rolling summary

Context compaction SHALL be incremental: a conversation's rolling summary and a
sequence watermark (the last message folded into it) SHALL be persisted and
reused, so that on a later turn or after a restart only the messages added since
the watermark are summarized and folded into the cached summary — not the whole
middle re-summarized from scratch. The model-facing context SHALL keep the same
shape as before (system prompt + rolling summary + the recent messages).

#### Scenario: a follow-up turn summarizes only the delta

- **WHEN** a conversation has a cached summary up to a watermark and more messages have since been added past it
- **THEN** compaction summarizes only the messages after the watermark, folds them into the cached summary, and advances the watermark — it does not re-summarize the already-summarized middle

#### Scenario: reload reuses the cache

- **WHEN** an over-limit conversation is reloaded after a restart
- **THEN** it reuses the persisted summary instead of re-summarizing the whole history

### Requirement: The cache is a safe, derived optimization

The compaction cache SHALL be a derived artifact, never the source of truth (the
full event stream remains authoritative). A missing, unparsable, or stale cache —
one whose watermark is ahead of the loaded history, or built under different
compaction config (recent-keep / threshold) — SHALL be ignored, falling back to a
full summarization and reseeding. The agent runtime SHALL perform no storage I/O
for this (it takes a cache in and returns the updated cache); persistence is the
server's responsibility.

#### Scenario: stale cache falls back safely

- **WHEN** the cache's watermark is ahead of the loaded history, or the compaction config has changed
- **THEN** the cache is ignored and a full summarization runs (today's behavior), reseeding the cache

#### Scenario: missing cache still works

- **WHEN** no cache exists for a conversation
- **THEN** compaction performs a full summarization and seeds the cache, with no error

#### Scenario: the core does no storage I/O

- **WHEN** the agent runtime compacts
- **THEN** it computes the updated cache and returns it without reading or writing storage; the server persists it
