# cached-context-compaction Specification

## Purpose

TBD - created by archiving change 'cached-context-compaction'. Update Purpose after archive.

## Requirements

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


<!-- @trace
source: cached-context-compaction
updated: 2026-06-29
code:
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/compress.rs
  - crates/fleety-server/src/tools.rs
  - crates/agent-core/src/event.rs
  - docs/env.md
  - crates/agent-core/src/agent.rs
  - crates/fleety-server/src/storage.rs
-->

---
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

<!-- @trace
source: cached-context-compaction
updated: 2026-06-29
code:
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/compress.rs
  - crates/fleety-server/src/tools.rs
  - crates/agent-core/src/event.rs
  - docs/env.md
  - crates/agent-core/src/agent.rs
  - crates/fleety-server/src/storage.rs
-->

---
### Requirement: The compaction budget measures only compactable history

The character budget that decides whether compaction runs SHALL measure only the portion of the context that compaction is able to compact. The leading, non-compactable preamble — the system prompt and any further system messages the caller places ahead of the conversation history — SHALL be excluded from that measurement.

Rationale, stated normatively: because the preamble is re-sent verbatim on every turn regardless of compaction, including it in the budget makes the threshold unconditionally exceeded and reduces the decision to a message-count check, which is not the intended gate.

The threshold value itself SHALL remain a fixed constant and SHALL NOT be exposed as an environment variable by this capability.

#### Scenario: a long preamble alone does not trigger compaction

- **WHEN** the leading non-compactable preamble is far larger than the threshold but the conversation history after it is smaller than the threshold
- **THEN** compaction does not run, the context is sent unchanged, and no summarization model call is made

##### Example: preamble dwarfs the threshold

- **GIVEN** a threshold of 24000 characters, a leading preamble totalling 58000 characters, and 12 history messages totalling 3000 characters
- **WHEN** the turn assembles its context
- **THEN** compaction does not run, because the compactable history measures 3000 characters

#### Scenario: history over the threshold still triggers compaction

- **WHEN** the conversation history after the preamble exceeds the threshold and there are more messages than the preamble plus the recent-keep window
- **THEN** compaction runs, summarizing the older middle and keeping the recent messages verbatim


<!-- @trace
source: context-budget-accounting
updated: 2026-08-01
code:
  - crates/fleety-eval/src/runner.rs
  - crates/agent-workflow/src/lib.rs
  - crates/agent-core/src/gemini.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/ondevice.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/wiki.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/model.rs
  - crates/agent-core/src/subagent.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/echo.rs
-->

---
### Requirement: The whole leading preamble survives compaction

Compaction SHALL preserve every leading system message verbatim, not only the first one. The preserved run SHALL be the maximal sequence of system messages at the head of the context. These messages SHALL NOT be folded into the summary, SHALL NOT be dropped, and SHALL appear ahead of the summary in the rebuilt context.

This requirement exists so that a caller which re-injects ephemeral per-turn context — the current time, an origin preamble, and injected instruction files — as leading system messages sees that context reach the model on every turn, as the instruction-file injection capability already requires.

#### Scenario: multiple leading system messages are all kept

- **WHEN** a context begins with several consecutive system messages and its compactable history exceeds the threshold
- **THEN** the rebuilt context contains all of those leading system messages unchanged, followed by the summary, followed by the recent messages

##### Example: five-message preamble preserved

- **GIVEN** a context of: system prompt, current-time system message, origin system message, local instruction-file system message, remote instruction-file system message, then 30 history messages over the threshold
- **WHEN** compaction runs
- **THEN** the rebuilt context is those 5 system messages, then one summary message, then the most recent history messages — and the current-time, origin, and instruction-file text appears in none of the summarized content

#### Scenario: re-injected preamble is never stale

- **WHEN** a conversation compacts on one turn and the caller re-injects a fresh preamble on the next turn
- **THEN** the model receives the fresh preamble, not preamble text captured inside an earlier summary

<!-- @trace
source: context-budget-accounting
updated: 2026-08-01
code:
  - crates/fleety-eval/src/runner.rs
  - crates/agent-workflow/src/lib.rs
  - crates/agent-core/src/gemini.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/ondevice.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/wiki.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/model.rs
  - crates/agent-core/src/subagent.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/echo.rs
-->