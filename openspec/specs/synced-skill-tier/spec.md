# synced-skill-tier Specification

## Purpose

TBD - created by archiving change 'synced-skill-tier'. Update Purpose after archive.

## Requirements

### Requirement: A synced skill tier updates from a repo at runtime

The server SHALL provide a fourth skill tier, `skills/synced`, populated at runtime from an external repository (default `TimLai666/skills`) rather than embedded in the binary, so its skills update without a Fleety release. A background task SHALL sync once at startup and then every configured interval. The synced tier SHALL have the lowest precedence: a skill of the same name in the installed, authored, or builtin tier SHALL override it. The tier SHALL be server-side only.

#### Scenario: synced skills are available without a release

- **WHEN** the sync runs and the repo has skill directories
- **THEN** those skills appear in `skills/synced` and are listed/usable, with no new Fleety binary

#### Scenario: a same-named higher tier wins

- **WHEN** a skill name exists in both the synced tier and the installed (or authored, or builtin) tier
- **THEN** the higher tier's skill is the one served


<!-- @trace
source: synced-skill-tier
updated: 2026-06-30
code:
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/skill_sync.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/builtin_skills.rs
  - docs/env.md
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/Cargo.toml
-->

---
### Requirement: Syncing is conditional on the repo's latest commit

Each sync SHALL first fetch the repository's latest commit SHA for the branch and compare it to the locally recorded last-synced SHA. The match short-circuit SHALL apply only while the local synced tier contains at least one skill directory: when the SHAs match and the tier is non-empty, the sync SHALL do nothing further (no download). When the SHAs differ, when no SHA is recorded yet, or when the local synced tier is empty (missing, or containing no skill directories), the sync SHALL download and apply the repo, then record the new SHA — so a tier emptied by an earlier fault self-heals on a later sync without waiting for a new upstream commit. Whether the tier is empty SHALL be a pure check on the synced directory.

#### Scenario: unchanged repo is not re-downloaded

- **WHEN** a sync runs, the remote latest SHA equals the recorded SHA, and the synced tier contains at least one skill
- **THEN** nothing is downloaded and the synced tier is unchanged

##### Example: sync decision

| local recorded SHA | remote latest SHA | local tier | download? |
|---|---|---|---|
| (none) | abc123 | (any) | yes |
| abc123 | abc123 | has skills | no (skip) |
| abc123 | def456 | has skills | yes |
| abc123 | abc123 | empty | yes (self-heal) |

#### Scenario: an emptied tier re-syncs despite an unchanged SHA

- **WHEN** the synced tier contains no skill directories (for example, a fault emptied it) but its recorded SHA still equals the remote latest SHA
- **THEN** the sync downloads and rebuilds the tier anyway, restoring the repo's skills without requiring a new upstream commit


<!-- @trace
source: skill-sync-empty-tier-retry
updated: 2026-07-11
code:
  - crates/fleety-server/src/skill_sync.rs
  - docs/env.md
-->

---
### Requirement: The synced tier mirrors the repo's skills, additions and removals

When a sync downloads the repo, it SHALL identify skills by a pruned recursive walk of the extracted tree: directories are visited top-down, dot-directories (names starting with `.`) are skipped, and the first directory along any path that contains a `SKILL.md` is a skill root — the walk SHALL NOT descend into a skill root, so a `SKILL.md` nested deeper inside belongs to that skill's content rather than defining another skill. A `SKILL.md` at the repo root itself SHALL be ignored (the repo root is never a skill, and loose files at any non-skill level are never skills). The synced tier SHALL be rebuilt from exactly the discovered set, flattened by skill directory name — so a skill added upstream appears and one removed upstream disappears (the tier is replaced atomically by the new set, so removals are inherent rather than diffed). When two skill roots share the same directory name, the one earliest in relative-path sort order SHALL be synced and the others SHALL be skipped with a logged warning; a name collision SHALL NOT fail the sync. Identifying which directories are skills SHALL be a pure function.

#### Scenario: a skill removed upstream is removed locally

- **WHEN** a skill directory that previously synced is no longer present in the repo, and a sync applies a newer commit
- **THEN** that skill directory is removed from the synced tier

#### Scenario: loose repo files are not skills

- **WHEN** the repo root contains files that are not inside a directory with a `SKILL.md`
- **THEN** they are ignored (not written as skills)

#### Scenario: skills in a plugin-marketplace layout are discovered

- **WHEN** the repo stores skills under a nested layout such as plugins/<plugin>/skills/<skill>/SKILL.md, with no top-level skill directories
- **THEN** each such skill directory is synced into the flat synced tier under its own directory name

##### Example: discovery across layouts

| Path in repo | Discovered as |
| ------------ | ------------- |
| a/SKILL.md | skill `a` (flat layout, unchanged) |
| plugins/p1/skills/b/SKILL.md | skill `b` |
| plugins/p1/skills/b/sub/SKILL.md | part of skill `b` (walk pruned at `b`) |
| .claude-plugin/x/SKILL.md | ignored (dot-directory) |
| SKILL.md (repo root) | ignored (root is never a skill) |

#### Scenario: a nested sub-skill stays inside its parent

- **WHEN** a synced skill's directory contains another `SKILL.md` in a subdirectory (a sub-skill shipped as part of the skill)
- **THEN** only the outer directory becomes a synced skill, and the nested `SKILL.md` is synced as part of the parent's content at its original relative path

#### Scenario: same-named skill roots collide deterministically

- **WHEN** two skill roots in different parts of the repo share the same directory name
- **THEN** the one earliest in relative-path sort order is synced, the others are skipped, a warning is logged, and the sync still succeeds


<!-- @trace
source: skill-sync-plugin-layout
updated: 2026-07-11
code:
  - docs/env.md
  - crates/fleety-server/src/skill_sync.rs
-->

---
### Requirement: Syncing never crashes and is configurable

Any failure during sync (network, API, download, unzip, I/O) SHALL be reported as a logged warning and leave the previously synced copy intact; it SHALL NOT crash the server. Downloaded content SHALL be assembled in a temporary location and swapped in only once complete, so a partially-synced state is never served. The repository, interval, and an on/off switch SHALL be configurable via environment variables; when disabled, no sync task runs.

#### Scenario: a sync failure keeps the last good copy

- **WHEN** a sync fails partway (e.g. the download errors)
- **THEN** the server logs a warning, the existing synced tier is unchanged, and nothing crashes

#### Scenario: syncing can be turned off

- **WHEN** the sync is disabled by its environment switch
- **THEN** no sync task runs and the synced tier is left as-is

<!-- @trace
source: synced-skill-tier
updated: 2026-06-30
code:
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/skill_sync.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/builtin_skills.rs
  - docs/env.md
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/Cargo.toml
-->