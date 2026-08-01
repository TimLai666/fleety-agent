## ADDED Requirements

### Requirement: Skill file reads return a single line-numbered view

`skill_read_file` SHALL return exactly one view of the requested slice: a line-numbered view, together with the skill name, its source tier, the in-skill file name, and the slice's start line, end line, and total line count. It SHALL NOT also return an unnumbered copy of the same slice.

This mirrors the workspace file-read behavior deliberately: both tools share the same character budget for tool results, so both SHALL spend it on content rather than on a duplicate of the same bytes. The tool description SHALL state that the line-number prefix is not part of the file content.

#### Scenario: a skill file read carries no duplicate copy

- **WHEN** `skill_read_file` returns successfully for a file inside a skill
- **THEN** the result contains the line-numbered view and no separate unnumbered copy of the same slice

#### Scenario: slice bounds are still reported

- **WHEN** `skill_read_file` is called with a start line and end line on a skill file
- **THEN** the result reports the numbered view of that slice together with its start line, end line, and the file's total line count
