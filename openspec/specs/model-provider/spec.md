# model-provider Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: OpenAI-compatible model endpoint

The runtime SHALL read `FLEETY_MODEL_BASE_URL` (the OpenAI-compatible `/v1` root), `FLEETY_MODEL` (the model name), and `FLEETY_MODEL_KEY` (the bearer token when the endpoint needs one). When `FLEETY_MODEL_BASE_URL` and `FLEETY_MODEL` are unset, the runtime SHALL fall back to a local echo provider rather than failing to start. `FLEETY_MODEL_STREAM` SHALL default to `0`; when set to `1` the runtime SHALL use the SSE streaming endpoint for token-by-token output.

#### Scenario: unset provider falls back to echo

- **WHEN** the server starts with `FLEETY_MODEL_BASE_URL` and `FLEETY_MODEL` unset
- **THEN** it runs with the echo provider instead of refusing to start

#### Scenario: streaming opt-in

- **WHEN** `FLEETY_MODEL_STREAM=1`
- **THEN** the runtime requests the SSE streaming endpoint

<!-- @trace
source: baseline-config-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-discuss.md
  - .spectra.yaml
  - CLAUDE.md
  - .opencode/commands/spectra-ask.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-debug/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
-->