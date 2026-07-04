## ADDED Requirements

### Requirement: Same-layer skill precedence favors .agents over .claude

At any single directory layer (a project cwd layer or the user-global layer), when a skill of the same name exists in both that layer's `.agents/skills` and `.claude/skills`, the `.agents/skills` version SHALL be the one served — the generic Agents standard overrides the Claude-specific one. This same-layer rule applies within every scope; the scope ordering (project > user > global tiers) and the depth ordering (deeper cwd layer > shallower) are unchanged.

#### Scenario: an .agents skill overrides a same-named .claude skill

- **WHEN** a directory layer holds a skill of the same name in both `.agents/skills` and `.claude/skills`
- **THEN** `list_skills` and `use_skill` serve the `.agents/skills` version
