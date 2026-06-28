## ADDED Requirements

### Requirement: Read and edit agent core memory files

The system SHALL provide `memory_read`, `memory_write`, and `memory_edit` operating on the agent core memory files only (`ME.md`, `USER.md`, `TODO.md`, `TOOLS.md`). `memory_read` SHALL return raw `content`, a `numbered` view, and `line_count`, with optional `start_line`/`end_line`. `memory_write` SHALL write a whole file with `mode` `replace` (default) or `append`. `memory_edit` SHALL support substring mode (`old`→`new`, unique unless `replace_all`) and line-range mode (`start_line`..`end_line`→`new`) and SHALL return the post-edit `applied` region. These tools SHALL NOT take a `device` argument; a device's notes are read via `device_show`.

#### Scenario: surgical edit returns the applied region

- **WHEN** `memory_edit` replaces a unique substring in `TODO.md`
- **THEN** the result reports the replacement and an `applied` line-numbered view of the change
