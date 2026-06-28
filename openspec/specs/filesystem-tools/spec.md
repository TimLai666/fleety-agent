# filesystem-tools Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Read and inspect workspace files

The system SHALL provide `read_file`, `list_dir`, and `search_files` tools. `read_file` SHALL return the raw `content`, a line-numbered `numbered` view, and `line_count`, and SHALL accept optional 1-based `start_line`/`end_line` to return a slice. `list_dir` SHALL list a directory (default `.`). `search_files` SHALL run a ripgrep search that respects `.gitignore` and skips binaries.

#### Scenario: read a slice with line numbers

- **WHEN** `read_file` is called with `start_line=2` and `end_line=3` on a file of 5 lines
- **THEN** `content` contains only lines 2-3, `numbered` shows those lines prefixed with their 1-based numbers, and `line_count` is 5


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

---
### Requirement: Mutate files with backup and rollback

The system SHALL provide `write_file`, `edit_file`, `delete_file`, `move_file`, `make_dir`, and `rollback`. Before overwriting or removing existing file content, `write_file`, `edit_file`, and `delete_file` SHALL copy the prior content into a managed backup store and return a `backup` with an `id`; `move_file` SHALL back up a destination it is about to overwrite. A `write_file` that creates a brand-new file SHALL return a null backup (no prior content), and `make_dir` creates a directory and has no prior content to back up. The backup store location SHALL be configured by the host (the server places it outside the workspace). `rollback` SHALL restore a file from a `backup_id`. `edit_file` SHALL support a substring mode (unique `old` → `new`) and a line-range mode (`start_line`..`end_line` → `new`), and SHALL return a unified `diff` plus an `applied` line-numbered view of the changed region.

#### Scenario: edit then roll back

- **WHEN** `edit_file` replaces a unique substring and a later `rollback` is called with the returned `backup.id`
- **THEN** the edit returns a `diff` and `applied` region, and the rollback restores the file to its pre-edit content


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

---
### Requirement: Filesystem scope and sensitive-path guard

By default the file tools SHALL resolve relative paths against the configured root and SHALL allow absolute paths and paths outside the root (audited and rollback-backed). When `FLEETY_FS_SCOPE=workspace` is set, the tools SHALL confine every path to the root and reject `..`, absolute, and symlink-escaping paths. Regardless of scope, the tools SHALL refuse **mutation** of critical paths (SSH keys/config, `/etc/shadow`, `/dev`, Windows system directories, and similar) with an actionable error; reads SHALL NOT be restricted by the sensitive-path guard.

#### Scenario: refuse a sensitive write but allow an outside-root write

- **WHEN** `write_file` targets an absolute path outside the root that is not sensitive
- **THEN** the write succeeds and is backed up
- **WHEN** `write_file` targets a sensitive path such as an SSH `authorized_keys`
- **THEN** the call is refused with an actionable critical-path error

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