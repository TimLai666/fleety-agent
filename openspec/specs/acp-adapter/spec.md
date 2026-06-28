# acp-adapter Specification

## Purpose

TBD - created by archiving change 'acp-adapter'. Update Purpose after archive.

## Requirements

### Requirement: Fleety runs as an ACP agent over stdio

The CLI SHALL provide an `acp` subcommand that runs an Agent Client Protocol agent: it reads JSON-RPC 2.0 messages from stdin and writes responses and notifications to stdout, so an ACP-capable editor can launch it as a subprocess. stdout SHALL carry only protocol messages; all logging SHALL go to stderr. Malformed input SHALL produce a JSON-RPC error response, never a crash or stray stdout output.

#### Scenario: editor drives a prompt turn

- **WHEN** an editor launches `fleety acp`, initializes, opens a session, and sends a prompt
- **THEN** the agent streams assistant output as `session/update` notifications and ends the turn with a `session/prompt` response carrying a stop reason

#### Scenario: malformed input is handled cleanly

- **WHEN** invalid JSON-RPC is received on stdin
- **THEN** the agent replies with a JSON-RPC error and keeps running, and writes nothing non-protocol to stdout


<!-- @trace
source: acp-adapter
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/conversation_lifecycle.rs
  - crates/fleety-server/src/workspace.rs
  - prompts/memory.md
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-daemon/src/main.rs
  - prompts/rules.md
  - crates/fleety-cli/Cargo.toml
  - Cargo.toml
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
-->

---
### Requirement: ACP methods map to the fleety-server agent

The adapter SHALL bridge ACP to the existing fleety-server rather than reimplementing the agent. It SHALL handle `initialize` (version + capability negotiation), `session/new`, `session/load`, `session/prompt`, and `session/cancel`, translating them to the server's conversation protocol and streaming the server's assistant output back as `session/update`. Unknown methods SHALL return a JSON-RPC method-not-found error.

#### Scenario: new session opens a server conversation rooted at the editor's directory

- **WHEN** the editor calls `session/new` with a working directory
- **THEN** a server conversation is opened whose working root is that directory (carried as the message origin), and an ACP session id is returned

#### Scenario: load resumes a conversation

- **WHEN** the editor calls `session/load` for a known session
- **THEN** the adapter resumes the mapped server conversation and replays its history as `session/update` notifications

#### Scenario: cancel stops the turn

- **WHEN** the editor sends `session/cancel` during a turn
- **THEN** the in-flight server turn is stopped and no further `session/update` for that turn is emitted


<!-- @trace
source: acp-adapter
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/conversation_lifecycle.rs
  - crates/fleety-server/src/workspace.rs
  - prompts/memory.md
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-daemon/src/main.rs
  - prompts/rules.md
  - crates/fleety-cli/Cargo.toml
  - Cargo.toml
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
-->

---
### Requirement: Tool approvals surface as ACP permission requests

When the server requests approval for a tool (under an approval-required policy), the adapter SHALL emit an ACP `session/request_permission` to the editor and translate the user's choice back into the server's approve/deny. Under the default full-access policy, no permission requests are raised.

#### Scenario: approval prompts the editor

- **WHEN** the server requests approval for a tool call during a turn
- **THEN** the adapter sends `session/request_permission` and, on the user's allow/deny, replies to the server accordingly

#### Scenario: full access raises no prompt

- **WHEN** the server runs under the default full-access policy
- **THEN** tool calls proceed without `session/request_permission`, as today

<!-- @trace
source: acp-adapter
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/conversation_lifecycle.rs
  - crates/fleety-server/src/workspace.rs
  - prompts/memory.md
  - crates/fleety-server/src/conversation_recall.rs
  - crates/fleety-daemon/src/main.rs
  - prompts/rules.md
  - crates/fleety-cli/Cargo.toml
  - Cargo.toml
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/tz.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/acp.rs
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
-->