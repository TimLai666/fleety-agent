# skills-management Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Three-tier skill store

The system SHALL load `SKILL.md` skill packs from three tiers — builtin (shipped, read-only), authored (written by the agent itself), and installed (user-chosen) — merging by name with precedence installed over authored over builtin. `list_skills` SHALL report each skill's `source` and on-disk `path`; `use_skill` SHALL return a skill's full contents AND its on-disk `path` (the skill's directory), so the agent can run a tool script the skill stores under `scripts/` via the command-execution tool.

#### Scenario: installed shadows builtin

- **WHEN** an installed skill and a builtin skill share a name and `list_skills` is called
- **THEN** the entry's `source` is `installed` and its `path` points at the installed copy

#### Scenario: use_skill returns the skill path

- **WHEN** `use_skill` loads a skill
- **THEN** the result includes the skill's directory `path` alongside its contents


<!-- @trace
source: skill-learning-loop
updated: 2026-06-28
code:
  - docs/env.md
  - prompts/protocol.md
  - prompts/rules.md
  - crates/fleety-server/builtin-skills/skill-creator/SKILL.md
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/conn.rs
  - docs/tools.md
  - crates/fleety-server/src/builtin_skills.rs
-->

---
### Requirement: File-level skill editing with tier rules

The system SHALL provide `skill_install`, `skill_remove`, `skill_list_files`, `skill_read_file`, `skill_write_file`, `skill_edit_file`, and `skill_delete_file` operating on individual files within a skill directory. Builtin skills SHALL NEVER be mutated. A write to a not-yet-existing skill SHALL land in the authored tier. In-skill file paths SHALL be rejected if they contain `..`, are absolute, or otherwise escape the skill directory.

#### Scenario: refuse to edit a builtin skill

- **WHEN** `skill_write_file` targets a file inside a builtin-only skill
- **THEN** the call is refused because builtin skills are read-only

#### Scenario: reject an escaping in-skill path

- **WHEN** `skill_read_file` is given a `file` containing `..`
- **THEN** the call is refused with an invalid-path error

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