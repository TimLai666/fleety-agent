# fleet-backup Specification

## Purpose

TBD - created by archiving change 'fleet-backup'. Update Purpose after archive.

## Requirements

### Requirement: Back up non-regenerable state to a user-configured private repo

The server SHALL back up its non-regenerable state to a Git repository configured by the user — never a hardcoded repo. The backup SHALL copy the agent home excluding regenerable/oversized paths (downloaded models, the builtin and synced skill tiers, and the rollback backups store) plus the `config.toml` and `providers.toml` files (which carry model API keys, in cleartext, per the user's choice), into a local git mirror, then `git add`/`commit`/`push`. Committing SHALL be a no-op when nothing changed (so unchanged state is not re-pushed). The repo and token come from environment/config (`FLEETY_BACKUP_REPO`, `FLEETY_BACKUP_TOKEN`); when no repo is configured, backup SHALL be entirely inactive (no loop, no mirror). Which paths are excluded SHALL be a pure function.

#### Scenario: unchanged state is not re-pushed

- **WHEN** a scheduled backup runs and nothing under the backed-up paths changed since the last backup
- **THEN** git produces no new commit and nothing is pushed

#### Scenario: regenerable and oversized paths are excluded

- **WHEN** the backup copies the agent home
- **THEN** downloaded models, the builtin skill tier, the synced skill tier, and the rollback backups store are not included, while conversations/memory/wiki/settings and API keys are

#### Scenario: no configured repo means no backup

- **WHEN** no backup repo is configured
- **THEN** no backup loop is spawned and no mirror is created


<!-- @trace
source: fleet-backup
updated: 2026-07-02
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/backup.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
-->

---
### Requirement: Refuse to push to a non-private repo

Before pushing, the server SHALL confirm via the GitHub API that the target repo is private. If it is not private, or its visibility cannot be determined, the server SHALL refuse to push, log a warning, and keep the local mirror (which still holds the committed snapshot). Parsing the repo visibility SHALL be a pure function.

##### Example: push decision by repo visibility

| API `private` field | push? |
|---|---|
| `true` | yes |
| `false` | no (refuse) |
| (missing / error) | no (refuse) |

#### Scenario: a public repo is never pushed to

- **WHEN** the configured repo is public
- **THEN** the backup does not push, logs a warning, and the cleartext secrets never leave the host


<!-- @trace
source: fleet-backup
updated: 2026-07-02
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/backup.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
-->

---
### Requirement: Runtime scheduling and manual trigger

When a backup repo is configured, the server SHALL back up on a schedule (every `FLEETY_BACKUP_INTERVAL_SECS`, default 3600) via a background task started at boot, and SHALL also expose a manual `backup now` server subcommand. Any failure (copy, git, network, API) SHALL be logged as a warning and leave the previous local mirror intact; it SHALL NOT crash the server.

#### Scenario: a failed backup does not crash

- **WHEN** a backup attempt fails partway (e.g. push auth error or network down)
- **THEN** the server logs a warning, the local mirror is intact, and nothing crashes


<!-- @trace
source: fleet-backup
updated: 2026-07-02
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/backup.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
-->

---
### Requirement: Restore from backup preserves existing data first

The server SHALL provide a `backup restore` subcommand (run on the server host while stopped) that clones the configured repo and restores its contents into place. Before overwriting, it SHALL rename the existing agent home and config files to timestamped `.pre-restore-<timestamp>` copies (kept, not deleted) so the operation is reversible, then place the backed-up contents and print a restart prompt. Restore SHALL NOT run automatically at boot. Computing the preserved path SHALL be a pure function.

#### Scenario: existing data is preserved, not destroyed

- **WHEN** `backup restore` runs with existing local state present
- **THEN** the current agent home and config files are renamed to `.pre-restore-<timestamp>` (kept) before the backup contents are put in place

<!-- @trace
source: fleet-backup
updated: 2026-07-02
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/backup.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
-->