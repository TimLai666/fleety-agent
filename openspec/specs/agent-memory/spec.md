# agent-memory Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Read and edit agent core memory files

The system SHALL provide `memory_read`, `memory_write`, and `memory_edit` operating on the agent core memory files only (`ME.md`, `USER.md`, `TODO.md`, `TOOLS.md`). `memory_read` SHALL return exactly one view of the requested slice: a line-numbered `numbered` view, together with `line_count`, and SHALL accept optional `start_line`/`end_line`. `memory_read` SHALL NOT also return an unnumbered copy of the same slice, because it shares the fixed tool-result character budget with the other slice-returning read tools and returning the same bytes twice halves how much reaches the model. Its tool description SHALL state that the line-number prefix is not part of the file content, so that a caller constructing an exact-text match for `memory_edit` knows to strip it. `memory_write` SHALL write a whole file with `mode` `replace` (default) or `append`. `memory_edit` SHALL support substring mode (`old`→`new`, unique unless `replace_all`) and line-range mode (`start_line`..`end_line`→`new`) and SHALL return the post-edit `applied` region. These tools SHALL NOT take a `device` argument; a device's notes are read via `device_show`.

The `applied` region returned by `memory_edit` is a post-edit confirmation view, not a slice read, and SHALL keep its existing shape.

#### Scenario: surgical edit returns the applied region

- **WHEN** `memory_edit` replaces a unique substring in `TODO.md`
- **THEN** the result reports the replacement and an `applied` line-numbered view of the change

#### Scenario: a memory read carries no duplicate unnumbered copy

- **WHEN** `memory_read` returns successfully for a core memory file
- **THEN** the result contains the numbered view and no separate unnumbered copy of the same slice

<!-- @trace
source: context-budget-accounting
updated: 2026-08-01
code:
  - crates/fleety-eval/src/runner.rs
  - crates/agent-workflow/src/lib.rs
  - crates/agent-core/src/gemini.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/ondevice.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/wiki.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/model.rs
  - crates/agent-core/src/subagent.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/echo.rs
-->