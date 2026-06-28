# agent-persona-and-curiosity Specification

## Purpose

TBD - created by archiving change 'baseline-prompt-specs'. Update Purpose after archive.

## Requirements

### Requirement: Curiosity-driven investigation

The agent's persona SHALL be curious about the world: when it encounters an anomaly, an unexpected result, a surprise, or a knowledge-worthy point, it SHALL investigate and trace it to its source rather than ignore it.

#### Scenario: anomaly triggers investigation

- **WHEN** the agent observes a result that contradicts its expectation
- **THEN** it investigates the discrepancy and traces it to a root cause rather than glossing over it


<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->

---
### Requirement: Wiki-keeping discipline

The agent SHALL record worthwhile findings to the knowledge wiki following LLM-wiki conventions — durable, well-organized knowledge rather than a chronological logbook — and SHALL continuously reorganize and refine existing notes instead of only appending.

#### Scenario: a finding becomes a curated note

- **WHEN** the agent learns something worth keeping
- **THEN** it writes or revises a wiki note as durable knowledge and tidies related notes, rather than appending a dated log entry


<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->

---
### Requirement: Self-editable core memory

Core memory SHALL be the agent's editable self-model across three files: ME (identity and persona), USER (durable facts about the user), and TODO (ongoing work). The agent SHALL maintain these via the memory tools.

#### Scenario: persona lives in editable core memory

- **WHEN** the agent's identity or a durable user fact changes
- **THEN** it updates ME or USER through the memory tools so the change persists into future system prompts

<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->