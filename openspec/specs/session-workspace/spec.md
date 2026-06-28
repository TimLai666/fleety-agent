# session-workspace Specification

## Purpose

TBD - created by archiving change 'session-workspace-cwd'. Update Purpose after archive.

## Requirements

### Requirement: Conversation works in the originating directory and device

A conversation's filesystem, command, and git tools SHALL operate, by default, in the directory the originating CLI was launched from (`origin.cwd`), executing on the originating device (the `device_id` from `Hello`). When that device is the server host, tools run there directly; when it is another registered executor device, tools run on that device via the existing device routing. The binding SHALL be resolved once from the conversation's first message and reused for subsequent turns.

#### Scenario: tools run in the CLI's directory on the CLI's device

- **WHEN** a user opens the CLI in directory D on device X and starts a conversation
- **THEN** that conversation's file/command/git tools read, write, and run in D on X

#### Scenario: two CLIs are independent

- **WHEN** one CLI runs in directory D on device X and another runs in directory E on device Y
- **THEN** each conversation is rooted at its own directory and device, independently

#### Scenario: a later message's different cwd does not move the conversation

- **WHEN** a conversation already has a resolved workspace binding and a later message carries a different cwd
- **THEN** the conversation stays anchored to the originally resolved directory and device (the change is ignored for routing)

##### Example: workspace + device resolution

| Origin cwd | Origin device vs server host | Resolved root | Executing device |
| ---------- | ---------------------------- | ------------- | ---------------- |
| absolute path D | same host | D | server host |
| absolute path D | other registered executor | D | that device |
| absolute path D | other device, no executor | fallback root | server host |
| blank / relative / none | any | fallback root | server host |


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