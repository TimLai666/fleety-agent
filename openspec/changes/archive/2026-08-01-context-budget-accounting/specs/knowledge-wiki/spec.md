## MODIFIED Requirements

### Requirement: Read and write the knowledge wiki

The system SHALL provide `wiki_write`, `wiki_read`, `wiki_list`, and `wiki_search`. `wiki_read` SHALL return exactly one view of the requested slice: a line-numbered view, together with the note's total line count and the slice's start and end lines, and SHALL accept optional `start_line`/`end_line`. `wiki_read` SHALL NOT also return an unnumbered copy of the same slice, because it shares the fixed tool-result character budget with the other slice-returning read tools and returning the same bytes twice halves how much reaches the model. Its tool description SHALL state that the line-number prefix is not part of the note content. `wiki_write` SHALL persist a note at a relative path inside the wiki vault. `wiki_search` SHALL run a literal/substring search across notes.

#### Scenario: read a wiki note slice

- **WHEN** `wiki_read` is called with `start_line`/`end_line` on a note
- **THEN** it returns a line-numbered view of the requested slice plus the note's total line count

#### Scenario: a wiki read carries no duplicate unnumbered copy

- **WHEN** `wiki_read` returns successfully for a note
- **THEN** the result contains the numbered view and no separate unnumbered copy of the same slice
