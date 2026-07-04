## MODIFIED Requirements

### Requirement: Conversation works in the originating directory and device

A conversation SHALL bind once, from its first message, to the originating directory (`origin.cwd`) and device (the `device_id` from `Hello`), and SHALL reuse that binding for subsequent turns and across resume. When the originating device is the server host and `origin.cwd` is a usable absolute path, the conversation's filesystem, command, and git tools SHALL be rooted at that cwd and run locally. When the originating device is not the server host, those tools SHALL remain rooted at the server workspace; the runtime SHALL NOT silently relocate tool execution to the origin device. Instead, the conversation SHALL rely on the injected origin context (see "Runtime injects origin context into each turn") so the agent routes work to the origin device via `device_exec`. A later message carrying a different cwd SHALL NOT move an already-bound conversation.

#### Scenario: tools run in the CLI's directory on the CLI's device

- **WHEN** a user opens the CLI in directory D on the server-host device and starts a conversation
- **THEN** that conversation's file/command/git tools read, write, and run locally in D

#### Scenario: two CLIs are independent

- **WHEN** one CLI runs in directory D on device X and another runs in directory E on device Y
- **THEN** each conversation is bound to its own directory and device, independently

#### Scenario: a later message's different cwd does not move the conversation

- **WHEN** a conversation already has a resolved workspace binding and a later message carries a different cwd
- **THEN** the conversation stays anchored to the originally bound directory and device (the change is ignored)

#### Scenario: cross-device tools stay server-rooted and defer to device_exec

- **WHEN** the originating device is not the server host
- **THEN** the conversation's tools stay rooted at the server workspace, and the runtime records the origin device and injects it (rather than auto-routing tool execution), so the agent uses `device_exec` to act on the origin device

##### Example: workspace + device resolution

| Origin cwd | Origin device vs server host | Resolved root | Where bare tools run |
| ---------- | ---------------------------- | ------------- | -------------------- |
| absolute path D | same host | D | server host, locally |
| absolute path D | other device | server fallback root | server host; origin device injected so the agent uses `device_exec` to reach D on that device |
| blank / relative / none | any | fallback root | server host |

## ADDED Requirements

### Requirement: Runtime injects origin context into each turn

The runtime SHALL inject the conversation's bound origin context — the originating device id, hostname, os, and cwd — into every turn as an ephemeral system preamble that is NOT written to the conversation history, so it survives long-context compaction and is re-presented each turn. The injected text SHALL distinguish the same-host case (tools already rooted at the cwd) from the cross-device case (bare tools run on the server, and the agent MUST use `device_exec(device=<id>)` to act on the origin device). The origin fields SHALL be persisted with the workspace binding so resume and subsequent turns re-present the same origin. When a conversation has no usable origin, the runtime SHALL omit the origin preamble and proceed on the server workspace.

#### Scenario: origin is re-presented every turn and survives compaction

- **WHEN** a bound conversation runs many turns and its earlier history is compacted into a rolling summary
- **THEN** each turn's system preamble still states the origin device and cwd, because it is re-injected per turn rather than stored in the compacted history

#### Scenario: cross-device origin drives device_exec and reading origin instructions

- **WHEN** the origin is another device X with cwd D, and the agent needs to read or modify files under D (including the per-level `AGENTS.md` / `CLAUDE.md` project instructions)
- **THEN** the injected context names X and D and directs the agent to `device_exec`, so the agent reads D's `AGENTS.md` / `CLAUDE.md` on X via `device_exec` rather than the server's local files

#### Scenario: origin persists across resume

- **WHEN** a bound conversation is resumed and the next message carries no origin
- **THEN** the origin preamble is rebuilt from the persisted binding and is unchanged from before the resume

#### Scenario: no usable origin omits the preamble

- **WHEN** a message has no usable origin (an older CLI that sends none, or a blank or relative cwd)
- **THEN** no origin preamble is injected and the turn proceeds on the server workspace
