## ADDED Requirements

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
