## ADDED Requirements

### Requirement: Enabled plugin skills join the conversation-scoped tiers

The `skills/` directory of each enabled Claude Code plugin on the originating device SHALL be included among that conversation's skill sources, in the scope where the plugin is enabled: a project-enabled plugin contributes to the project scope, a user-enabled plugin to the user scope. Plugin-provided skills SHALL rank below directly-placed `.agents` / `.claude` skills within the same scope, and follow the overall precedence (project > user > global tiers). This reuses the existing conversation-scoped overlay — it adds skill sources, not a new tier.

#### Scenario: an enabled plugin's skill is available in the conversation

- **WHEN** a same-host conversation binds and a project- or user-enabled plugin has a skill under its `skills/` directory
- **THEN** that skill appears in `list_skills` for that conversation, ranked below same-scope directly-placed skills

#### Scenario: a directly-placed skill outranks a same-named plugin skill

- **WHEN** the same scope has a same-named skill both directly (in `.agents/skills` or `.claude/skills`) and inside an enabled plugin
- **THEN** the directly-placed skill is the one served
