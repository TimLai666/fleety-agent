# acp-editor-delegation Specification

## Purpose

TBD - created by archiving change 'acp-editor-delegation'. Update Purpose after archive.

## Requirements

### Requirement: Editor-backed tools execute in the user's editor

In a conversation served by an editor, the agent SHALL be offered named
editor-backed tools — text read/write/edit over the editor's filesystem
(buffer-aware) and a command runner over the editor's terminal — that execute in
the user's editor on its host. These tools SHALL be named so their execution
surface is unambiguous and SHALL NOT reroute or rename the server's own tools.
They SHALL be offered only for capabilities the editor advertises; an
unadvertised capability SHALL fall back to the server's disk tools.

#### Scenario: a buffer edit shows in the editor

- **WHEN** the agent calls the editor write/edit tool in an editor-served conversation
- **THEN** the change is applied through the editor (appearing in the user's buffer with the editor's approval/diff), and the result reports the editor surface

#### Scenario: commands run in the editor's terminal

- **WHEN** the editor advertises terminal support and the agent runs a command via the editor runner
- **THEN** it executes in the editor's terminal on the editor's host and returns output and exit status

#### Scenario: missing capability falls back

- **WHEN** the editor does not advertise a capability (e.g. terminal)
- **THEN** the corresponding editor tool is not offered and the agent uses the server's disk tool instead


<!-- @trace
source: acp-editor-delegation
updated: 2026-06-29
code:
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-server/src/wiki_embed.rs
  - crates/fleety-server/src/storage.rs
  - Cargo.toml
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-server/src/embed.rs
  - crates/fleety-server/src/editor_tools.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/device.rs
  - crates/agent-workflow/src/lib.rs
  - crates/fleety-server/src/bridge.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/conversation_embed.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
-->

---
### Requirement: Editor tools target this conversation's editor, identified by host and connection

An editor connection SHALL be treated as a per-conversation execution channel, not
a device. The editor-backed tools SHALL target the editor serving the current
conversation. The connection SHALL report the device (host) it runs on, and SHALL
be addressable as a specific connection so that multiple editors on one host do
not collide.

#### Scenario: multiple editors on one host don't cross-talk

- **WHEN** two editor connections run on the same host and serve different conversations
- **THEN** a tool call for one conversation reaches that conversation's editor, not the other's

#### Scenario: the editor reports its host

- **WHEN** an editor session connects
- **THEN** it declares the device (host) it runs on, so editor and disk operations are understood to be on the same machine


<!-- @trace
source: acp-editor-delegation
updated: 2026-06-29
code:
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-server/src/wiki_embed.rs
  - crates/fleety-server/src/storage.rs
  - Cargo.toml
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-server/src/embed.rs
  - crates/fleety-server/src/editor_tools.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/device.rs
  - crates/agent-workflow/src/lib.rs
  - crates/fleety-server/src/bridge.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/conversation_embed.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
-->

---
### Requirement: The agent prefers editor tools and is told how surfaces differ

In an editor-served conversation the agent SHALL be guided to prefer the
editor-backed tools for the user's work, and SHALL be told how the surface differs
(editor edits live in the buffer, may be unsaved and require the user's approval,
and disk reads do not reflect unsaved buffer edits until saved). This guidance
SHALL be carried in the editor tools' own descriptions (so it travels with the
tools and needs no separate prompt change), and editor tool results SHALL carry
their surface and saved state.

#### Scenario: the editor tools steer the agent to prefer them

- **WHEN** a conversation is served by an editor
- **THEN** each editor tool's description instructs the agent to prefer it for the user's files and explains the buffer/unsaved surface, so the agent reasons correctly

#### Scenario: results disclose surface and saved state

- **WHEN** the agent performs an editor write
- **THEN** the result states it landed in the editor buffer and whether it is saved, so the agent does not treat it as persisted to disk

<!-- @trace
source: acp-editor-delegation
updated: 2026-06-29
code:
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-server/src/wiki_embed.rs
  - crates/fleety-server/src/storage.rs
  - Cargo.toml
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-server/src/embed.rs
  - crates/fleety-server/src/editor_tools.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/device.rs
  - crates/agent-workflow/src/lib.rs
  - crates/fleety-server/src/bridge.rs
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/conversation_embed.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
-->