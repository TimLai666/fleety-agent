# conversation-recall Specification

## Purpose

TBD - created by archiving change 'conversation-recall'. Update Purpose after archive.

## Requirements

### Requirement: Conversation events carry a timestamp

Each stored conversation event SHALL record a wall-clock timestamp (`ts_secs`) alongside its sequence number. The field SHALL be additive and backward compatible: existing records without it SHALL still load (read as an unknown/zero time), and resume/replay behavior SHALL be unchanged.

#### Scenario: new events are timestamped, old ones still load

- **WHEN** a new message is appended and an older record without a timestamp is read back
- **THEN** the new record carries its write-time `ts_secs`, the old record loads with an unknown (zero) time, and replay returns events in sequence order as before


<!-- @trace
source: conversation-recall
updated: 2026-06-29
code:
  - prompts/memory.md
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - docs/env.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-protocol/src/lib.rs
  - prompts/policy.md
  - crates/fleety-server/src/workspace.rs
-->

---
### Requirement: The agent can search its past conversations

The agent SHALL have tools to search the conversations of the device it is acting for: a keyword search and a semantic search. Results SHALL include the conversation id, sequence, timestamp, role, and a snippet, ordered so the agent can tell precedence and time (keyword newest-first; semantic by relevance but carrying timestamps). A listing tool SHALL report the device's conversations with their last-activity time. Recall SHALL be scoped to that device's conversations.

#### Scenario: keyword recall returns time-ordered hits

- **WHEN** the agent runs the keyword conversation search for a term that appears in past conversations
- **THEN** it gets matches with conversation id, sequence, timestamp, role, and snippet, ordered newest-first

#### Scenario: semantic recall finds related exchanges

- **WHEN** the agent runs the semantic conversation search for a query
- **THEN** it gets the most relevant past exchanges, each carrying its timestamp and sequence so their time order is known

#### Scenario: listing shows recency

- **WHEN** the agent lists the device's conversations
- **THEN** each is shown with its last-activity timestamp


<!-- @trace
source: conversation-recall
updated: 2026-06-29
code:
  - prompts/memory.md
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - docs/env.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-protocol/src/lib.rs
  - prompts/policy.md
  - crates/fleety-server/src/workspace.rs
-->

---
### Requirement: Recall is best-effort and degrades without embeddings

Conversation indexing SHALL run in the background and SHALL NOT block or fail a turn. When the semantic embedding model is unavailable (embeddings disabled), semantic search SHALL degrade to keyword search with a note rather than error. Conversations that predate the index SHALL be backfilled lazily, with keyword search serving results in the meantime.

#### Scenario: indexing never blocks a turn

- **WHEN** a message is appended during a turn
- **THEN** the turn proceeds without waiting on indexing, and an indexing failure does not affect the turn

#### Scenario: semantic search without a model

- **WHEN** semantic conversation search is invoked while embeddings are disabled
- **THEN** it returns keyword results with a note that semantic search is unavailable, and does not error

<!-- @trace
source: conversation-recall
updated: 2026-06-29
code:
  - prompts/memory.md
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - docs/env.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-protocol/src/lib.rs
  - prompts/policy.md
  - crates/fleety-server/src/workspace.rs
-->

---
### Requirement: Semantic recall is embedding-backed

`conversation_semantic_search` SHALL be backed by the per-user conversation
embedding index (cosine-ranked results), not a keyword alias, while remaining
scoped to the acting user and degrading to keyword search when embeddings are
unavailable. The keyword tools (`conversation_search`, `conversation_list`) are
unchanged and remain the always-available fallback.

#### Scenario: semantic search is no longer a keyword alias

- **WHEN** embeddings are enabled and the acting user's index has content
- **THEN** `conversation_semantic_search` returns embedding-ranked results, not the keyword result

#### Scenario: keyword tools unchanged

- **WHEN** the acting user calls `conversation_search` or `conversation_list`
- **THEN** they behave exactly as before (no embedding dependency)

<!-- @trace
source: conversation-embedding-recall
updated: 2026-06-29
code:
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/conversation_embed.rs
  - docs/env.md
  - crates/fleety-server/src/wiki_embed.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/embed.rs
-->