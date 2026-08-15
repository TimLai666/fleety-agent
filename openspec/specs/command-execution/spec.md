# command-execution Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Run shell commands with a critical-command guard

The system SHALL provide a `run_command` tool that runs one shell command (default working directory the root, overridable by `cwd`) and returns `stdout`, `stderr`, and `exit_code`. It SHALL detect clearly irreversible command shapes, including disk wipe, `mkfs`, `dd` to a device, `rm -rf /`, host shutdown/reboot, and similar patterns. Under `full_access` or `require_approval`, a detected critical command SHALL be refused with an actionable error before execution. Under `auto_review`, the detector SHALL emit a trusted danger signal to the reviewer and SHALL NOT refuse the command before that review; the command SHALL execute only after a valid reviewer approval.

#### Scenario: ordinary command runs

- **WHEN** `run_command` is given `echo hi`
- **THEN** it returns `exit_code` 0 and the captured output

#### Scenario: default policy refuses a catastrophic command

- **WHEN** `run_command` is given `rm -rf /` under `full_access`
- **THEN** it is refused with a critical-command error and is not executed

#### Scenario: auto review evaluates a catastrophic command

- **WHEN** `run_command` is given `rm -rf /` under `auto_review`
- **THEN** the reviewer receives a catastrophic-delete danger signal and the command executes only if the reviewer approves


<!-- @trace
source: auto-review
updated: 2026-08-15
code:
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/agent-core/src/agent.rs
  - docs/tools.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/event.rs
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: Diff files a command changed

`run_command` SHALL accept an optional `track` array of paths and return a unified before/after `diff` for each, so file changes a command makes are observable.

#### Scenario: track a file the command edits

- **WHEN** `run_command` runs a command that appends to a tracked file
- **THEN** the result includes a diff showing the appended line for that path

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