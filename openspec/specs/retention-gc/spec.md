# retention-gc Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: Periodic retention sweep

The server SHALL run a periodic background sweep that bounds the audit and backup surfaces. `FLEETY_GC_DISABLED` SHALL, when set to any value, skip the loop entirely. `FLEETY_GC_INTERVAL_SECS` SHALL set the sweep cadence (default `21600`, i.e. 6 hours) and SHALL be clamped to a 60-second floor. `FLEETY_BACKUPS_RETENTION_SECS` SHALL set the maximum backup age before deletion (default `604800`, i.e. 7 days). `FLEETY_HISTORY_ROTATE_BYTES` SHALL set the size at which a device's `history.jsonl` is rotated to an archive and reset (default `33554432`, i.e. 32 MiB).

#### Scenario: sweep deletes aged backups and rotates oversized history

- **WHEN** a sweep runs with defaults and a backup directory is older than 7 days
- **THEN** that backup directory is deleted
- **WHEN** a device's `history.jsonl` exceeds 32 MiB during a sweep
- **THEN** it is renamed to a timestamped archive and the live file resets

#### Scenario: interval is floored

- **WHEN** `FLEETY_GC_INTERVAL_SECS` is set below 60
- **THEN** the effective cadence is clamped to 60 seconds

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