## MODIFIED Requirements

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
