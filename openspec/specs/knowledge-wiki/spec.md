# knowledge-wiki Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Read and write the knowledge wiki

The system SHALL provide `wiki_write`, `wiki_read`, `wiki_list`, and `wiki_search`. `wiki_read` SHALL return exactly one view of the requested slice: a line-numbered view, together with the note's total line count and the slice's start and end lines, and SHALL accept optional `start_line`/`end_line`. `wiki_read` SHALL NOT also return an unnumbered copy of the same slice, because it shares the fixed tool-result character budget with the other slice-returning read tools and returning the same bytes twice halves how much reaches the model. Its tool description SHALL state that the line-number prefix is not part of the note content. `wiki_write` SHALL persist a note at a relative path inside the wiki vault. `wiki_search` SHALL run a literal/substring search across notes.

#### Scenario: read a wiki note slice

- **WHEN** `wiki_read` is called with `start_line`/`end_line` on a note
- **THEN** it returns a line-numbered view of the requested slice plus the note's total line count

#### Scenario: a wiki read carries no duplicate unnumbered copy

- **WHEN** `wiki_read` returns successfully for a note
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

---
### Requirement: Local semantic search over the wiki

The system SHALL provide `wiki_semantic_search` that embeds the `query` with a local EmbeddingGemma 300M model and returns the `top_k` most similar note chunks by cosine distance from an on-disk vector index. The index SHALL stay current automatically, re-embedding notes whose content hash changed. When semantic search is disabled by configuration, the tool SHALL return an actionable error pointing at `wiki_search` rather than failing silently.

#### Scenario: semantic query returns ranked chunks

- **WHEN** `wiki_semantic_search` is called with a `query` and `top_k=3` against an indexed vault
- **THEN** it returns up to 3 note chunks ordered by ascending cosine distance

#### Scenario: disabled embedding gives an actionable error

- **WHEN** `wiki_semantic_search` is called while embedding is disabled by configuration
- **THEN** it returns an error directing the caller to use `wiki_search`

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