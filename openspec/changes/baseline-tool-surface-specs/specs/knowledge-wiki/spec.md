## ADDED Requirements

### Requirement: Read and write the knowledge wiki

The system SHALL provide `wiki_write`, `wiki_read`, `wiki_list`, and `wiki_search`. `wiki_read` SHALL return raw content, a line-numbered view, and line count, with optional `start_line`/`end_line`. `wiki_write` SHALL persist a note at a relative path inside the wiki vault. `wiki_search` SHALL run a literal/substring search across notes.

#### Scenario: read a wiki note slice

- **WHEN** `wiki_read` is called with `start_line`/`end_line` on a note
- **THEN** it returns the requested slice plus a line-numbered view and the note's total line count

### Requirement: Local semantic search over the wiki

The system SHALL provide `wiki_semantic_search` that embeds the `query` with a local EmbeddingGemma 300M model and returns the `top_k` most similar note chunks by cosine distance from an on-disk vector index. The index SHALL stay current automatically, re-embedding notes whose content hash changed. When semantic search is disabled by configuration, the tool SHALL return an actionable error pointing at `wiki_search` rather than failing silently.

#### Scenario: semantic query returns ranked chunks

- **WHEN** `wiki_semantic_search` is called with a `query` and `top_k=3` against an indexed vault
- **THEN** it returns up to 3 note chunks ordered by ascending cosine distance

#### Scenario: disabled embedding gives an actionable error

- **WHEN** `wiki_semantic_search` is called while embedding is disabled by configuration
- **THEN** it returns an error directing the caller to use `wiki_search`
