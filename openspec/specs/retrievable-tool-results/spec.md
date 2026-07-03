# retrievable-tool-results Specification

## Purpose

TBD - created by archiving change 'retrievable-tool-results'. Update Purpose after archive.

## Requirements

### Requirement: Truncated tool results are locatable

Whenever a tool result loses content on the way to the model — whether the
character budget truncated it OR the structural compression dropped array items
or string characters — the marker SHALL include the result's id (the tool-call
id that keys the retained event), so the agent can request the full result
precisely. When no id is available the marker SHALL fall back to today's id-less
wording. A result that fits the budget and lost nothing SHALL be returned
unchanged with no marker.

#### Scenario: a truncated result names how to fetch it

- **WHEN** a tool result exceeds the budget and is truncated with an id available
- **THEN** the marker names the id to pass to the retrieval tool

#### Scenario: a within-budget result that was structurally compressed names how to fetch it

- **WHEN** a tool result fits the character budget but its structural compression dropped array items or string characters
- **THEN** the output still carries a marker naming the id to fetch the full result

#### Scenario: results that lost nothing are untouched

- **WHEN** a tool result is within the budget and its structural compression dropped nothing
- **THEN** it is returned unchanged with no marker


<!-- @trace
source: retrievable-tool-results
updated: 2026-06-29
code:
  - crates/agent-core/src/compress.rs
  - docs/env.md
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/event.rs
  - crates/agent-core/src/agent.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/tools.rs
-->

---
### Requirement: Full tool results are retrievable in bounded segments

The system SHALL provide a `fetch_tool_result(id, offset?, limit?)` tool that
returns the full retained result for an id as a character window
(`offset`..`offset`+`limit`), reporting the total length and the next offset
(null at the end). A single fetch SHALL NOT exceed the standard tool-result
character budget — `limit` defaults to and is capped at that budget — so a large
result can only be read by paging, and retrieval can never re-exceed the context
budget the truncation protected.

#### Scenario: paging through a large result

- **WHEN** the agent calls `fetch_tool_result` for a large result and follows `next_offset` until it is null
- **THEN** it receives the whole result across bounded segments, each within the budget, with an accurate total length

#### Scenario: a fetch cannot blow the budget

- **WHEN** a caller requests a `limit` larger than the tool-result budget
- **THEN** the returned window is clamped to the budget

#### Scenario: offset past the end

- **WHEN** the offset is at or beyond the total length
- **THEN** the content is empty, the next offset is null, and the reported total is correct

<!-- @trace
source: retrievable-tool-results
updated: 2026-06-29
code:
  - crates/agent-core/src/compress.rs
  - docs/env.md
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/event.rs
  - crates/agent-core/src/agent.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/tools.rs
-->