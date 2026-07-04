## ADDED Requirements

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
