# skills-management Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

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


<!-- @trace
source: conversation-scoped-skills
updated: 2026-07-04
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/skill_sources.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/skills.rs
  - crates/agent-core/src/compress.rs
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/storage.rs
-->

---
### Requirement: File-level skill editing with tier rules

The system SHALL provide `skill_install`, `skill_remove`, `skill_list_files`, `skill_read_file`, `skill_write_file`, `skill_edit_file`, and `skill_delete_file` operating on individual files within a skill directory. Builtin skills SHALL NEVER be mutated. A write to a not-yet-existing skill SHALL land in the authored tier. In-skill file paths SHALL be rejected if they contain `..`, are absolute, or otherwise escape the skill directory.

#### Scenario: refuse to edit a builtin skill

- **WHEN** `skill_write_file` targets a file inside a builtin-only skill
- **THEN** the call is refused because builtin skills are read-only

#### Scenario: reject an escaping in-skill path

- **WHEN** `skill_read_file` is given a `file` containing `..`
- **THEN** the call is refused with an invalid-path error

<!-- @trace
source: baseline-tool-surface-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - CLAUDE.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-discuss.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-ask.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-drift/SKILL.md
-->

---
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

<!-- @trace
source: conversation-scoped-skills
updated: 2026-07-04
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/skill_sources.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/skills.rs
  - crates/agent-core/src/compress.rs
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/storage.rs
-->

---
### Requirement: Same-layer skill precedence favors .agents over .claude

At any single directory layer (a project cwd layer or the user-global layer), when a skill of the same name exists in both that layer's `.agents/skills` and `.claude/skills`, the `.agents/skills` version SHALL be the one served — the generic Agents standard overrides the Claude-specific one. This same-layer rule applies within every scope; the scope ordering (project > user > global tiers) and the depth ordering (deeper cwd layer > shallower) are unchanged.

#### Scenario: an .agents skill overrides a same-named .claude skill

- **WHEN** a directory layer holds a skill of the same name in both `.agents/skills` and `.claude/skills`
- **THEN** `list_skills` and `use_skill` serve the `.agents/skills` version

<!-- @trace
source: agents-over-claude-precedence
updated: 2026-07-04
code:
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/skill_sources.rs
  - crates/fleety-server/src/instructions.rs
-->

---
### Requirement: Enabled plugin skills join the conversation-scoped tiers

The `skills/` directory of each enabled Claude Code plugin on the originating device SHALL be included among that conversation's skill sources, in the scope where the plugin is enabled: a project-enabled plugin contributes to the project scope, a user-enabled plugin to the user scope. Plugin-provided skills SHALL rank below directly-placed `.agents` / `.claude` skills within the same scope, and follow the overall precedence (project > user > global tiers). This reuses the existing conversation-scoped overlay — it adds skill sources, not a new tier.

#### Scenario: an enabled plugin's skill is available in the conversation

- **WHEN** a same-host conversation binds and a project- or user-enabled plugin has a skill under its `skills/` directory
- **THEN** that skill appears in `list_skills` for that conversation, ranked below same-scope directly-placed skills

#### Scenario: a directly-placed skill outranks a same-named plugin skill

- **WHEN** the same scope has a same-named skill both directly (in `.agents/skills` or `.claude/skills`) and inside an enabled plugin
- **THEN** the directly-placed skill is the one served

<!-- @trace
source: claude-plugin-declarative-reuse
updated: 2026-07-04
code:
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/skill_sources.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/plugin_sources.rs
  - crates/fleety-server/src/mcp.rs
  - crates/fleety-server/src/main.rs
-->

---
### Requirement: Skill file reads return a single line-numbered view

`skill_read_file` SHALL return exactly one view of the requested slice: a line-numbered view, together with the skill name, its source tier, the in-skill file name, and the slice's start line, end line, and total line count. It SHALL NOT also return an unnumbered copy of the same slice.

This mirrors the workspace file-read behavior deliberately: both tools share the same character budget for tool results, so both SHALL spend it on content rather than on a duplicate of the same bytes. The tool description SHALL state that the line-number prefix is not part of the file content.

#### Scenario: a skill file read carries no duplicate copy

- **WHEN** `skill_read_file` returns successfully for a file inside a skill
- **THEN** the result contains the line-numbered view and no separate unnumbered copy of the same slice

#### Scenario: slice bounds are still reported

- **WHEN** `skill_read_file` is called with a start line and end line on a skill file
- **THEN** the result reports the numbered view of that slice together with its start line, end line, and the file's total line count

<!-- @trace
source: context-budget-accounting
updated: 2026-08-01
code:
  - crates/fleety-eval/src/runner.rs
  - crates/agent-workflow/src/lib.rs
  - crates/agent-core/src/gemini.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/ondevice.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/wiki.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/model.rs
  - crates/agent-core/src/subagent.rs
  - crates/agent-core/src/agent.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/echo.rs
-->