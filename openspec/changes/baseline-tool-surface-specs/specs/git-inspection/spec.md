## ADDED Requirements

### Requirement: Read-only git inspection

The system SHALL provide read-only git tools: `git_status`, `git_diff`, `git_log` (optional `limit`), and `git_show` (optional `ref`, default `HEAD`). `git_diff` SHALL show the unstaged working-tree diff and MUST also include untracked new files so changes from any source are visible. These tools SHALL NOT mutate the repository.

#### Scenario: diff includes an untracked file

- **WHEN** a new untracked file exists and `git_diff` is called
- **THEN** the result lists the untracked file alongside the unstaged diff
