## ADDED Requirements

### Requirement: Per-user conversation vector index

The system SHALL maintain a per-user conversation vector index (sqlite-vec) built
with the local embedding model, one embedding per message, stored under the user's
own directory so it inherits the user privacy boundary. The index SHALL be updated
incrementally off the turn (never blocking the user's turn), with a bounded
backfill from the user's stored conversations when the index is missing or behind.
One user's index SHALL never be read for another user.

#### Scenario: new messages become searchable after the turn

- **WHEN** a turn completes for an acting user
- **THEN** that turn's new messages are embedded into the user's index off the turn, so a later semantic search can find them

#### Scenario: index is per-user

- **WHEN** semantic search runs for a given acting user
- **THEN** only that user's index is consulted; no other user's conversations are reachable

### Requirement: Semantic conversation search, with keyword fallback

`conversation_semantic_search` SHALL embed the query and return the acting user's
most semantically similar past messages ranked by cosine similarity (newest-first
on ties), each result carrying the similarity score plus conversation id, sequence,
timestamp, role, and snippet. When embeddings are disabled or the index is empty or
unavailable, it SHALL fall back to keyword search (the prior behavior) without
error. A guest (no identified user) SHALL receive nothing.

#### Scenario: ranked by similarity

- **WHEN** embeddings are enabled and the index has content
- **THEN** results are ordered by cosine similarity (newest-first on ties) with the score populated

#### Scenario: degrades to keyword

- **WHEN** embeddings are disabled or the index is empty/unavailable
- **THEN** the tool returns the keyword result without error, never worse than today

#### Scenario: guest gets nothing

- **WHEN** there is no identified acting user
- **THEN** semantic search returns no results
