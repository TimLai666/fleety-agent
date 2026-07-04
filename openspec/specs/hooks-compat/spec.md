# hooks-compat Specification

## Purpose

TBD - created by archiving change 'claude-hooks-compat'. Update Purpose after archive.

## Requirements

### Requirement: Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks

The runtime SHALL, for a conversation, discover the originating device's Claude Code `PreToolUse` and `PostToolUse` hooks declared in the user settings (`~/.claude/settings.json`) and the project settings (project `.claude/settings.json`), and run the matching hooks around that conversation's tool calls. Hooks SHALL be parsed from the `hooks.PreToolUse` and `hooks.PostToolUse` arrays, where each array element carries a `matcher` (a tool-name pattern; absent or empty means match-all) and a `hooks` list whose entries of `type == "command"` provide the shell `command`. Parsing SHALL be best-effort: a missing or malformed settings file, an absent `hooks` section, or an entry without a command SHALL be skipped and SHALL NOT abort the conversation.

Hook matching SHALL use simple tool-name comparison for the first release: a `matcher` of `*` (or empty) matches every tool, otherwise the matcher SHALL match a tool whose name equals it. Advanced matcher syntax (regular expressions, tool-input predicates) is out of scope.

#### Scenario: PreToolUse and PostToolUse hooks are parsed

- **WHEN** a conversation's settings declare `hooks.PreToolUse` and `hooks.PostToolUse` entries with a `matcher` and a `command`
- **THEN** those hooks are collected, each tagged with its source scope (user or project) and its event

#### Scenario: malformed settings are skipped best-effort

- **WHEN** a settings file is missing, is not valid JSON, or has no `hooks` section
- **THEN** no hooks are collected from that source and the conversation proceeds without error

#### Scenario: matcher matches by tool name

- **WHEN** a hook's `matcher` is `*` or empty
- **THEN** it matches every tool
- **WHEN** a hook's `matcher` is a specific tool name
- **THEN** it matches only the tool whose name equals that matcher


<!-- @trace
source: claude-hooks-compat
updated: 2026-07-04
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/hooks_compat.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
-->

---
### Requirement: Run hooks around tool calls on the origin device

The runtime SHALL run a matching `PreToolUse` hook before a tool executes and a matching `PostToolUse` hook after it returns, without modifying the agent core. Each hook's shell command SHALL execute on the originating device: when the origin is the same host, the command SHALL run locally; when the origin is another device, the command SHALL be sent to that device for execution (via the cross-device command-execution path), because the hook belongs to the origin environment.

#### Scenario: PreToolUse runs before, PostToolUse runs after

- **WHEN** a tool whose name matches a collected hook is about to execute
- **THEN** the PreToolUse hook command runs first, and after the tool returns the matching PostToolUse hook command runs

#### Scenario: cross-device hook runs on the origin

- **WHEN** the conversation's origin is another device and a matching hook fires
- **THEN** the hook command is executed on the origin device rather than on the serving host


<!-- @trace
source: claude-hooks-compat
updated: 2026-07-04
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/hooks_compat.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
-->

---
### Requirement: PreToolUse hooks can deny a tool call

The runtime SHALL treat a `PreToolUse` hook that exits non-zero as a denial of that tool call: the tool SHALL NOT execute, and the agent SHALL receive a tool result indicating the call was denied by a hook, consistent with the existing approval-denial result shape. A `PostToolUse` hook SHALL NOT deny (the tool has already run); a failing PostToolUse hook SHALL only be recorded and SHALL NOT block the tool result from returning to the agent.

#### Scenario: non-zero PreToolUse exit denies the tool

- **WHEN** a matching PreToolUse hook command exits non-zero
- **THEN** the tool does not execute and the agent receives a hook-denied tool result

#### Scenario: failing PostToolUse does not block the result

- **WHEN** a matching PostToolUse hook command fails
- **THEN** the failure is recorded and the tool's result is still returned to the agent


<!-- @trace
source: claude-hooks-compat
updated: 2026-07-04
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/hooks_compat.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
-->

---
### Requirement: Hook executions are audited and project hooks are governed

The runtime SHALL record every hook shell execution (its command, outcome, and source scope) in the audit log. Hooks SHALL default to enabled (opt-out). User-scope hooks SHALL run by default. Project-scope hooks SHALL also run by default but SHALL be disableable independently via the `FLEETY_DISABLE_PROJECT_HOOKS` environment variable, and every project-scope hook execution SHALL be tagged in the audit log as project-sourced, because project settings may originate from an untrusted repository (a supply-chain risk).

#### Scenario: every hook execution is audited

- **WHEN** a hook command executes
- **THEN** the audit log records the command, its outcome, and whether its scope is user or project

#### Scenario: project hooks can be disabled independently

- **WHEN** `FLEETY_DISABLE_PROJECT_HOOKS` is set to `1`
- **THEN** project-scope hooks are not loaded or run, while user-scope hooks continue to run

#### Scenario: project hook executions are tagged project-sourced

- **WHEN** a project-scope hook executes
- **THEN** its audit record is tagged as project-sourced

<!-- @trace
source: claude-hooks-compat
updated: 2026-07-04
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/hooks_compat.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
-->

---
### Requirement: Hooks apply to subagent tool calls

The runtime SHALL apply a conversation's collected `PreToolUse` / `PostToolUse` hooks not only to the primary conversation's tools but also to the tools of any subagent spawned within that conversation, including nested subagents. A subagent's tool call SHALL be subject to the same hooks — with the same denial, best-effort, audit, and env-policy semantics — as the primary conversation's tool call, so that a safety hook cannot be bypassed by delegating the tool call to a subagent. When a conversation has no collected hooks, subagent registries SHALL be left unwrapped (unchanged behavior).

#### Scenario: a subagent tool call is denied by a PreToolUse hook

- **WHEN** a conversation has a `PreToolUse` hook matching a tool whose command exits non-zero, and the agent delegates that tool to a subagent
- **THEN** the subagent's tool call is denied and does not execute, consistent with the primary conversation's denial behavior

#### Scenario: nested subagents inherit the hooks

- **WHEN** a subagent spawns a further subagent within the same conversation
- **THEN** the nested subagent's tools are wrapped with the same hooks

#### Scenario: no hooks leaves subagent registries unwrapped

- **WHEN** a conversation has no collected hooks
- **THEN** subagent tool registries are not wrapped and behave as before

<!-- @trace
source: hooks-subagent-coverage
updated: 2026-07-04
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
-->

---
### Requirement: Lifecycle-event hooks (UserPromptSubmit, Stop, SubagentStop)

The runtime SHALL, in addition to the tool-scoped `PreToolUse` / `PostToolUse` hooks, discover and run a conversation's `UserPromptSubmit`, `Stop`, and `SubagentStop` hooks declared in the origin device's Claude Code settings. `UserPromptSubmit` hooks SHALL run when a user prompt is submitted, before the prompt is processed; a `UserPromptSubmit` hook that exits non-zero SHALL block that prompt from being processed (consistent with `PreToolUse` denial). `Stop` hooks SHALL run after the agent has finished handling a user message, and `SubagentStop` hooks SHALL run when a subagent finishes; these two SHALL run best-effort and audited but SHALL NOT block or force continuation in the first release. Every lifecycle-event hook execution SHALL be audited and SHALL honor the same env policy as tool hooks. When a conversation declares none of these events, behavior SHALL be unchanged.

#### Scenario: a non-zero UserPromptSubmit hook blocks the prompt

- **WHEN** a conversation has a `UserPromptSubmit` hook whose command exits non-zero and the user submits a prompt
- **THEN** that prompt is not processed and the user is told a hook blocked it

#### Scenario: UserPromptSubmit success lets the prompt through

- **WHEN** a `UserPromptSubmit` hook command exits zero
- **THEN** the prompt is processed normally, and the hook execution is audited

#### Scenario: Stop runs after a user message is handled

- **WHEN** the agent finishes handling a user message and a `Stop` hook is present
- **THEN** the `Stop` hook command runs and is audited, without changing the reply already produced

#### Scenario: SubagentStop runs when a subagent finishes

- **WHEN** a subagent finishes and a `SubagentStop` hook is present
- **THEN** the `SubagentStop` hook command runs and is audited

<!-- @trace
source: hooks-lifecycle-events
updated: 2026-07-05
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/hooks_compat.rs
  - crates/fleety-server/src/subagent.rs
-->