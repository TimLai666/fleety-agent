## ADDED Requirements

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
