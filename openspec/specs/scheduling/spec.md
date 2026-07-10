# scheduling Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Schedule prompts to run later

The system SHALL provide `schedule_create`, `schedule_list`, and `schedule_delete`. `schedule_create` SHALL accept a `trigger` (one-shot timestamp or recurring cron with optional `tz`) and a `prompt`, and SHALL accept an optional `mandate` and `allowed_tools` captured at creation time. `schedule_list` SHALL show each schedule's timezone and next fire time. The fire loop SHALL run a schedule only when the current time matches its trigger.

#### Scenario: cron schedule reports its next fire time

- **WHEN** `schedule_create` registers a cron trigger with a `tz` and `schedule_list` is called
- **THEN** the listing shows that schedule's timezone and computed next fire time

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
### Requirement: Record each run's outcome

The scheduler SHALL write a `last_outcome` record onto a schedule after every unattended run, for both successful and failed runs. The record MUST contain a `status` (`ok` or `error`), a one-line `summary` (the truncated final assistant output on success, the truncated error report on failure), and a `ts` (unix seconds). A run whose `run_turn` returns an error MUST be isolated: the scheduler MUST record an `error` outcome, mark the schedule fired, and continue to the remaining due schedules; a single failing schedule SHALL NOT abort the tick and SHALL NOT be retried on every subsequent tick.

#### Scenario: successful run records an ok outcome

- **WHEN** a due schedule's unattended run completes successfully during a tick
- **THEN** the schedule's `last_outcome.status` is `ok`, `summary` reflects the run's output, and the schedule is marked fired

#### Scenario: a failing schedule is recorded and isolated

- **WHEN** one due schedule's run fails while another due schedule in the same tick succeeds
- **THEN** both schedules are marked fired, each gets a `last_outcome` with the matching `status` (`error` and `ok`), and the failed schedule's `at:` trigger is not due on the next tick


<!-- @trace
source: schedule-run-notification
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/schedules.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - scripts/install.sh
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Surface last run outcome in schedule_list

`schedule_list` SHALL include each schedule's `last_run` and `last_outcome` (when the schedule has run at least once) so a user can see when it last ran and whether it succeeded or failed.

#### Scenario: schedule_list shows the last outcome

- **WHEN** a schedule has been run and `schedule_list` is called
- **THEN** the listing entry for that schedule includes `last_run` and a `last_outcome` carrying its `status`, `summary`, and `ts`


<!-- @trace
source: schedule-run-notification
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/schedules.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - scripts/install.sh
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Proactively notify the owner on next connect

When a device owned by the scheduler's user connects, the server SHALL deliver, as `ServerMsg::Assistant` messages, each schedule outcome completed since it was last notified, with failures prominently marked, and SHALL advance each delivered schedule's notification watermark so the same outcome is not delivered twice. The server SHALL NOT deliver these notifications to a Guest connection or to a device whose acting user differs from the scheduler's owner.

#### Scenario: owner receives unnotified outcomes once

- **WHEN** an owner device connects while a schedule has an outcome newer than its notification watermark
- **THEN** the owner receives one assistant message for that outcome (a failure is clearly marked and points at `schedule-<id>`), and a subsequent connect from that device does not redeliver the same outcome

#### Scenario: non-owner connection receives no schedule notifications

- **WHEN** a Guest connection (or a device whose acting user is not the scheduler's owner) connects with schedule outcomes pending
- **THEN** no schedule-outcome assistant messages are delivered to that connection

<!-- @trace
source: schedule-run-notification
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/schedules.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - scripts/install.sh
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->