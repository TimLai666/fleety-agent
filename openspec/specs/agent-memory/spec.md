# agent-memory Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Read and edit agent core memory files

The system SHALL provide `memory_read`, `memory_write`, and `memory_edit` operating on the agent core memory files only (`ME.md`, `USER.md`, `TODO.md`, `TOOLS.md`). `memory_read` SHALL return raw `content`, a `numbered` view, and `line_count`, with optional `start_line`/`end_line`. `memory_write` SHALL write a whole file with `mode` `replace` (default) or `append`. `memory_edit` SHALL support substring mode (`old`→`new`, unique unless `replace_all`) and line-range mode (`start_line`..`end_line`→`new`) and SHALL return the post-edit `applied` region. These tools SHALL NOT take a `device` argument; a device's notes are read via `device_show`.

#### Scenario: surgical edit returns the applied region

- **WHEN** `memory_edit` replaces a unique substring in `TODO.md`
- **THEN** the result reports the replacement and an `applied` line-numbered view of the change

<!-- @trace
source: baseline-tool-surface-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - CLAUDE.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-discuss.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-ask.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-drift/SKILL.md
-->