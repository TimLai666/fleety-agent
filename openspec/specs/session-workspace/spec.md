# session-workspace Specification

## Purpose

TBD - created by archiving change 'session-workspace-cwd'. Update Purpose after archive.

## Requirements

### Requirement: Conversation works in the originating directory and device

A conversation SHALL bind once, from its first message, to the originating directory (`origin.cwd`) and device (the `device_id` from `Hello`), and SHALL reuse that binding for subsequent turns and across resume. The originating device SHALL count as the server host only when the connection was accepted through explicit auth-required loopback trust; a client-supplied hostname or a loopback socket alone SHALL NOT establish that authority. When that trusted same-host decision holds and `origin.cwd` is a usable absolute path, the conversation's filesystem, command, and git tools SHALL be rooted at that cwd and run locally. Auth-disabled, token-authenticated, non-loopback, and loopback-trust-disabled sessions SHALL remain rooted at the server workspace and record the origin device; the runtime SHALL NOT silently relocate tool execution to the origin device. Instead, the conversation SHALL rely on the injected origin context (see "Runtime injects origin context into each turn") so the agent routes work to the origin device via `device_exec`. A later message carrying a different cwd SHALL NOT move an already-bound conversation.

#### Scenario: tools run in the CLI's directory on the CLI's device

- **WHEN** a user opens an unpaired CLI in directory D through an auth-required Server's explicitly trusted loopback path and starts a conversation
- **THEN** that conversation's file/command/git tools read, write, and run locally in D

#### Scenario: loopback alone does not grant a client cwd local authority

- **WHEN** an auth-disabled, token-authenticated, or loopback-trust-disabled connection supplies an absolute `origin.cwd`, even when its socket peer is loopback
- **THEN** the conversation remains rooted at the server fallback workspace and records the origin device

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
| absolute path D | explicitly loopback-trusted | D | server host, locally |
| absolute path D | any other session | server fallback root | server host; origin device injected so the agent uses `device_exec` to reach D on that device |
| blank / relative / none | any | fallback root | server host |


<!-- @trace
source: session-workspace-origin-injection
updated: 2026-07-04
code:
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
-->

---
### Requirement: Origin cwd is treated as untrusted

The server SHALL treat `origin.cwd` as untrusted client input: it SHALL reject a blank, relative, or non-absolute cwd (falling back instead), and binding to a cwd SHALL NOT bypass the active filesystem scope posture or the sensitive-path guard. Existing full-access behavior is preserved — the agent MAY still operate outside the resolved root — but the default root presented to the model becomes the validated cwd.

#### Scenario: a non-absolute cwd is rejected

- **WHEN** a message's `origin.cwd` is blank or relative
- **THEN** the server does not bind to it and uses the fallback workspace instead

#### Scenario: the sensitive-path guard still applies

- **WHEN** a conversation is rooted at a directory and the agent targets a guarded/sensitive path
- **THEN** the tool call is refused by the existing sensitive-path guard regardless of the root


<!-- @trace
source: session-workspace-cwd
updated: 2026-06-29
code:
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-server/src/main.rs
  - prompts/memory.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/subagent.rs
  - prompts/policy.md
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
-->

---
### Requirement: Falls back to the server workspace when origin is absent

When a conversation has no usable origin binding — an older CLI that sends no origin, a blank/invalid cwd, or an origin device that is not a reachable executor — the server SHALL fall back to its existing workspace behavior: `FLEETY_WORKSPACE` if set, otherwise the server process's working directory, executing on the server host. The resolution precedence SHALL be origin cwd (on the origin device), then `FLEETY_WORKSPACE`, then the server cwd.

#### Scenario: older client keeps working

- **WHEN** a `UserMessage` arrives with no `origin`
- **THEN** tools use the server-side workspace exactly as before, and the conversation proceeds

#### Scenario: origin device not reachable

- **WHEN** the originating device has no connected executor (no fleetyd)
- **THEN** the server falls back to the server-side workspace and logs that on-origin execution is unavailable


<!-- @trace
source: session-workspace-cwd
updated: 2026-06-29
code:
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-server/src/main.rs
  - prompts/memory.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/subagent.rs
  - prompts/policy.md
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
-->

---
### Requirement: The workspace binding persists across resume

The resolved workspace binding (root and optional executing device) SHALL be stored with the conversation and reloaded on resume, so reconnecting continues in the same directory on the same device. If the bound device is offline at resume time, the server SHALL fall back rather than failing the resume.

#### Scenario: resume continues in the same tree

- **WHEN** a conversation with a resolved binding is resumed
- **THEN** its tools rebind to the same root and device and continue working there

#### Scenario: resume with an offline bound device

- **WHEN** a conversation is resumed but its bound device is offline
- **THEN** the resume succeeds using the fallback workspace and logs the downgrade

<!-- @trace
source: session-workspace-cwd
updated: 2026-06-29
code:
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-server/src/main.rs
  - prompts/memory.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/subagent.rs
  - prompts/policy.md
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
-->

---
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

<!-- @trace
source: session-workspace-origin-injection
updated: 2026-07-04
code:
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
-->
