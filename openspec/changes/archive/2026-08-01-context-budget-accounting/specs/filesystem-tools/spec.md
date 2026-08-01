## MODIFIED Requirements

### Requirement: Read and inspect workspace files

The system SHALL provide `read_file`, `list_dir`, and `search_files` tools. `read_file` SHALL return exactly one view of the requested slice: a line-numbered `numbered` view, together with `start_line`, `end_line`, and `line_count`. `read_file` SHALL NOT also return an unnumbered copy of the same slice, because a tool result carries a fixed character budget and returning the same bytes twice halves how much of a file reaches the model. `read_file` SHALL accept optional 1-based `start_line`/`end_line` to return a slice. The tool description SHALL state that the line-number prefix is not part of the file content, so that a caller constructing an exact-text match for an edit knows to strip it. `list_dir` SHALL list a directory (default `.`). `search_files` SHALL run a ripgrep search that respects `.gitignore` and skips binaries.

#### Scenario: read a slice with line numbers

- **WHEN** `read_file` is called with `start_line=2` and `end_line=3` on a file of 5 lines
- **THEN** `numbered` shows only lines 2-3 prefixed with their 1-based numbers, `start_line` is 2, `end_line` is 3, and `line_count` is 5

#### Scenario: the result carries no duplicate unnumbered copy

- **WHEN** `read_file` returns successfully for any path
- **THEN** the result contains the numbered view and no separate unnumbered copy of the same slice
