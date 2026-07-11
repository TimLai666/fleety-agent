## MODIFIED Requirements

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
