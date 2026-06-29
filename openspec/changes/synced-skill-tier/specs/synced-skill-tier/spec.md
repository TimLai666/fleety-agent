## ADDED Requirements

### Requirement: A synced skill tier updates from a repo at runtime

The server SHALL provide a fourth skill tier, `skills/synced`, populated at runtime from an external repository (default `TimLai666/skills`) rather than embedded in the binary, so its skills update without a Fleety release. A background task SHALL sync once at startup and then every configured interval. The synced tier SHALL have the lowest precedence: a skill of the same name in the installed, authored, or builtin tier SHALL override it. The tier SHALL be server-side only.

#### Scenario: synced skills are available without a release

- **WHEN** the sync runs and the repo has skill directories
- **THEN** those skills appear in `skills/synced` and are listed/usable, with no new Fleety binary

#### Scenario: a same-named higher tier wins

- **WHEN** a skill name exists in both the synced tier and the installed (or authored, or builtin) tier
- **THEN** the higher tier's skill is the one served

### Requirement: Syncing is conditional on the repo's latest commit

Each sync SHALL first fetch the repository's latest commit SHA for the branch and compare it to the locally recorded last-synced SHA. If they match, the sync SHALL do nothing further (no download). If they differ — or no SHA is recorded yet — it SHALL download and apply the repo, then record the new SHA.

##### Example: sync decision

| local recorded SHA | remote latest SHA | download? |
|---|---|---|
| (none) | abc123 | yes |
| abc123 | abc123 | no (skip) |
| abc123 | def456 | yes |

#### Scenario: unchanged repo is not re-downloaded

- **WHEN** a sync runs and the remote latest SHA equals the recorded SHA
- **THEN** nothing is downloaded and the synced tier is unchanged

### Requirement: The synced tier mirrors the repo's skills, additions and removals

When a sync downloads the repo, it SHALL treat each top-level directory that contains a `SKILL.md` as a skill (ignoring loose files at the repo root) and rebuild the synced tier from exactly that set — so a skill added upstream appears and one removed upstream disappears (the tier is replaced atomically by the new set, so removals are inherent rather than diffed). Identifying which directories are skills SHALL be a pure function.

#### Scenario: a skill removed upstream is removed locally

- **WHEN** a skill directory that previously synced is no longer present in the repo, and a sync applies a newer commit
- **THEN** that skill directory is removed from the synced tier

#### Scenario: loose repo files are not skills

- **WHEN** the repo root contains files that are not inside a directory with a `SKILL.md`
- **THEN** they are ignored (not written as skills)

### Requirement: Syncing never crashes and is configurable

Any failure during sync (network, API, download, unzip, I/O) SHALL be reported as a logged warning and leave the previously synced copy intact; it SHALL NOT crash the server. Downloaded content SHALL be assembled in a temporary location and swapped in only once complete, so a partially-synced state is never served. The repository, interval, and an on/off switch SHALL be configurable via environment variables; when disabled, no sync task runs.

#### Scenario: a sync failure keeps the last good copy

- **WHEN** a sync fails partway (e.g. the download errors)
- **THEN** the server logs a warning, the existing synced tier is unchanged, and nothing crashes

#### Scenario: syncing can be turned off

- **WHEN** the sync is disabled by its environment switch
- **THEN** no sync task runs and the synced tier is left as-is
