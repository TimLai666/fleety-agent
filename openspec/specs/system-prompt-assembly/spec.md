# system-prompt-assembly Specification

## Purpose

TBD - created by archiving change 'baseline-prompt-specs'. Update Purpose after archive.

## Requirements

### Requirement: Assemble the system prompt from embedded docs and core memory

The system message SHALL be built by embedding, at build time, `protocol.md`, then `rules.md`, then `memory.md`, then `policy.md`, joined by a `---` separator, followed by a `# Core Memory` section containing the agent's core memory (ME, USER, TODO). The four behavioural docs SHALL be compiled into the binary so the running agent never depends on the prompt files being present on disk.

#### Scenario: full prompt ordering

- **WHEN** the system prompt is assembled with `FLEETY_SYSTEM_PROMPT` unset
- **THEN** it contains protocol, rules, memory, and policy in that order, then a `# Core Memory` section with ME/USER/TODO


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
### Requirement: Preserve the system prompt across compaction

The assembled system prompt SHALL be placed at message index 0 and SHALL be preserved by context compaction, so it survives a context summary WITHOUT being re-sent as a separate per-turn reminder.

#### Scenario: prompt survives a summary

- **WHEN** the context is compacted mid-conversation
- **THEN** the index-0 system prompt is retained and is not duplicated as an extra reminder message


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
### Requirement: Minimal mode drops the static docs

When `FLEETY_SYSTEM_PROMPT=minimal` is set, the system prompt SHALL contain only the core memory (ME/USER/TODO) and SHALL omit the four embedded behavioural docs, for token-lean or debugging runs.

#### Scenario: minimal keeps only core memory

- **WHEN** `FLEETY_SYSTEM_PROMPT=minimal`
- **THEN** the system prompt is the core memory alone, without protocol/rules/memory/policy

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