## MODIFIED Requirements

### Requirement: Three-tier skill store

The system SHALL load `SKILL.md` skill packs from three tiers — builtin (shipped, read-only), authored (written by the agent itself), and installed (user-chosen) — merging by name with precedence installed over authored over builtin. `list_skills` SHALL report each skill's `source`, its on-disk `path`, AND the device that path lives on (`device`: null for the server host, otherwise the device id, aligned with the workspace binding's device representation). `use_skill` SHALL return a skill's full contents, its on-disk `path` (the skill's directory), AND that path's `device`, so the agent can run a tool script the skill stores under `scripts/` via the command-execution tool on the correct device (a bare `path` without its device is not a usable handle when the skill lives on another device).

#### Scenario: installed shadows builtin

- **WHEN** an installed skill and a builtin skill share a name and `list_skills` is called
- **THEN** the entry's `source` is `installed` and its `path` points at the installed copy

#### Scenario: use_skill returns the skill path

- **WHEN** `use_skill` loads a skill
- **THEN** the result includes the skill's directory `path` alongside its contents

#### Scenario: list and use report the skill's device

- **WHEN** `list_skills` or `use_skill` returns a skill
- **THEN** the result includes a `device` field identifying where the skill's `path` lives (null for the server host, otherwise the device id)

## ADDED Requirements

### Requirement: Conversation-scoped project and user skill tiers

The system SHALL, per conversation, overlay two conversation-scoped skill tiers on top of the global tiers: a `project` tier collected from the origin path's own `.claude/skills` and `.agents/skills` directories (each level from the origin cwd upward), and a `user` tier collected from the originating device's user-global `~/.claude/skills` and `~/.agents/skills`. These conversation-scoped tiers SHALL take precedence over all global tiers, giving the order project > user > installed > authored > builtin > synced. The conversation-scoped skills SHALL be visible only within that conversation (they SHALL NOT enter the global skill store and SHALL NOT be visible to other conversations); this is achieved by overlaying the sources only onto that conversation's per-connection registry. In the first release the conversation-scoped tiers SHALL be collected only when the origin is on the server host (device null); when the origin is another device or absent, the conversation-scoped tiers SHALL be empty and the conversation SHALL fall back to the global tiers.

#### Scenario: a project skill from the origin path is available in that conversation

- **WHEN** a conversation binds on the server host with an origin cwd whose directory chain contains a `.claude/skills` or `.agents/skills` skill
- **THEN** that skill appears in `list_skills` for that conversation with its `device` being null (server host)

#### Scenario: conversation-scoped skills are isolated

- **WHEN** conversation A binds to a project with its own skills and conversation B binds elsewhere
- **THEN** B's `list_skills` does not include A's conversation-scoped skills, and neither enters the global skill store

#### Scenario: a conversation-scoped skill overrides a same-named global skill

- **WHEN** a skill name exists both in the conversation's project tier and in the global installed tier
- **THEN** `list_skills` / `use_skill` serve the project-tier skill

#### Scenario: cross-device or absent origin falls back to global tiers

- **WHEN** the origin is another device, or the message has no usable origin
- **THEN** no conversation-scoped tiers are collected and only the global tiers are served

##### Example: tier precedence

| Skill present in | Served from |
| ---------------- | ----------- |
| project + installed + builtin | project |
| user + installed | user |
| installed + builtin | installed |
| synced only | synced |
