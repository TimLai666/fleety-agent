# per-user-timezone Specification

## Purpose

TBD - created by archiving change 'per-user-timezone'. Update Purpose after archive.

## Requirements

### Requirement: Times are presented in the acting user's timezone

The agent SHALL be told the current time in the acting user's timezone at the start of a turn, and timestamps it presents (audit, recall, listings) SHALL be rendered in that timezone. The timezone SHALL resolve as: the acting user's configured IANA timezone, else a global `FLEETY_TZ`, else UTC. An invalid timezone value SHALL fall through to the next source rather than erroring.

#### Scenario: a user in a non-UTC zone sees local times

- **WHEN** the acting user has a configured timezone and the agent reports a timestamp or the current time
- **THEN** it is rendered in that user's timezone

#### Scenario: fallback when no user timezone

- **WHEN** the acting user has no timezone configured
- **THEN** times use `FLEETY_TZ` if set, otherwise UTC

#### Scenario: invalid timezone falls through

- **WHEN** a configured timezone string is not a valid IANA zone
- **THEN** resolution falls through to the next source (env, then UTC) without erroring


<!-- @trace
source: per-user-timezone
updated: 2026-06-29
code:
  - crates/fleety-server/src/identity.rs
  - prompts/policy.md
  - docs/env.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/conn.rs
  - prompts/memory.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/storage.rs
-->

---
### Requirement: Stored timestamps remain UTC

This change SHALL NOT alter how timestamps are stored: they remain Unix epoch (UTC). Timezone handling SHALL be a rendering concern only — applied when presenting a time or telling the agent the current time, never when writing.

#### Scenario: storage is unaffected

- **WHEN** an event with a timestamp is written
- **THEN** the stored value is the Unix epoch as before, regardless of any user timezone

<!-- @trace
source: per-user-timezone
updated: 2026-06-29
code:
  - crates/fleety-server/src/identity.rs
  - prompts/policy.md
  - docs/env.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/conn.rs
  - prompts/memory.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/storage.rs
-->