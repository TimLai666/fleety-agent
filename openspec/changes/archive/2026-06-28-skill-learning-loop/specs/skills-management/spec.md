## MODIFIED Requirements

### Requirement: Three-tier skill store

The system SHALL load `SKILL.md` skill packs from three tiers — builtin (shipped, read-only), authored (written by the agent itself), and installed (user-chosen) — merging by name with precedence installed over authored over builtin. `list_skills` SHALL report each skill's `source` and on-disk `path`; `use_skill` SHALL return a skill's full contents AND its on-disk `path` (the skill's directory), so the agent can run a tool script the skill stores under `scripts/` via the command-execution tool.

#### Scenario: installed shadows builtin

- **WHEN** an installed skill and a builtin skill share a name and `list_skills` is called
- **THEN** the entry's `source` is `installed` and its `path` points at the installed copy

#### Scenario: use_skill returns the skill path

- **WHEN** `use_skill` loads a skill
- **THEN** the result includes the skill's directory `path` alongside its contents
