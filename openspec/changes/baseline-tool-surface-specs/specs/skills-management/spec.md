## ADDED Requirements

### Requirement: Three-tier skill store

The system SHALL load `SKILL.md` skill packs from three tiers — builtin (shipped, read-only), authored (written by the agent itself), and installed (user-chosen) — merging by name with precedence installed over authored over builtin. `list_skills` SHALL report each skill's `source` and on-disk `path`; `use_skill` SHALL return a skill's full contents.

#### Scenario: installed shadows builtin

- **WHEN** an installed skill and a builtin skill share a name and `list_skills` is called
- **THEN** the entry's `source` is `installed` and its `path` points at the installed copy

### Requirement: File-level skill editing with tier rules

The system SHALL provide `skill_install`, `skill_remove`, `skill_list_files`, `skill_read_file`, `skill_write_file`, `skill_edit_file`, and `skill_delete_file` operating on individual files within a skill directory. Builtin skills SHALL NEVER be mutated. A write to a not-yet-existing skill SHALL land in the authored tier. In-skill file paths SHALL be rejected if they contain `..`, are absolute, or otherwise escape the skill directory.

#### Scenario: refuse to edit a builtin skill

- **WHEN** `skill_write_file` targets a file inside a builtin-only skill
- **THEN** the call is refused because builtin skills are read-only

#### Scenario: reject an escaping in-skill path

- **WHEN** `skill_read_file` is given a `file` containing `..`
- **THEN** the call is refused with an invalid-path error
