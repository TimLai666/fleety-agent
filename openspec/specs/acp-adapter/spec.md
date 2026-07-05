# acp-adapter Specification

## Purpose

Let an ACP-capable editor (Zed, …) drive Fleety from its agent panel. `fleety acp`
is a thin stdio bridge that maps the Agent Client Protocol to the fleety-server
conversation protocol — the real agent (model, tools, memory) runs in the server.
Verified end-to-end against Zed 1.9.

## Requirements

### Requirement: Fleety runs as an ACP agent over stdio

The CLI SHALL provide an `acp` subcommand that runs an Agent Client Protocol agent: it exchanges JSON-RPC 2.0 messages over stdio, **delimited by newlines — one JSON object per line, with no Content-Length headers and no embedded newlines, per the ACP transport** — so an ACP-capable editor can launch it as a subprocess. stdout SHALL carry only protocol messages; all logging SHALL go to stderr. Malformed input SHALL produce a JSON-RPC error response (parse error), never a crash or stray stdout output; a failed operation SHALL use the internal-error code, not the `-32000` code some editors treat as "authentication required".

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

The adapter SHALL bridge ACP to the existing fleety-server rather than reimplementing the agent. It SHALL handle `initialize` (version + capability negotiation), `session/new`, `session/load`, `session/prompt`, and `session/cancel`, translating them to the server's conversation protocol and streaming the server's assistant output back as `session/update` notifications — each tagged by `sessionUpdate: "agent_message_chunk"` and carrying a text content block, as ACP editors require. `session/cancel` SHALL be translated to the server's `CancelTurn` frame (it is a notification and gets no direct response); the session SHALL be marked cancelled so the in-flight `session/prompt` completes with `stopReason: "cancelled"` once the server's cancelled turn closes, instead of the normal `end_turn`. Unknown methods SHALL return a JSON-RPC method-not-found error; inbound frames with no `method` (an editor's response/error) SHALL be ignored, not answered.

#### Scenario: new session opens a server conversation rooted at the editor's directory

- **WHEN** the editor calls `session/new` with a working directory
- **THEN** a server conversation is opened whose working root is that directory (carried as the message origin), and an ACP session id is returned

#### Scenario: load resumes a conversation

- **WHEN** the editor calls `session/load` for a known session
- **THEN** the adapter resumes the mapped server conversation and replays its history as `session/update` notifications

#### Scenario: cancel stops the turn

- **WHEN** the editor sends `session/cancel` during a turn
- **THEN** the adapter forwards `CancelTurn` to the server, the in-flight server turn stops at its next checkpoint, and the pending `session/prompt` responds with `stopReason: "cancelled"`

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

---
### Requirement: The adapter is a bidirectional, persistent ACP agent

The ACP adapter SHALL act as a bidirectional ACP agent — both answering the
editor's requests (session methods) and calling the editor's filesystem and
terminal methods — over a connection that persists for the session and services
routed tool calls while a turn is streaming. It SHALL advertise the editor-backed
tools it can service (gated by the editor's advertised capabilities) and SHALL
report the device (host) the editor session runs on. Conformant editors require no
changes.

#### Scenario: a routed tool call is serviced mid-turn

- **WHEN** the server routes an editor tool call to the adapter while a turn is streaming
- **THEN** the adapter translates it to the matching ACP editor request, awaits the editor's response, and returns the tool result without interrupting the stream

#### Scenario: only advertised editor capabilities are offered

- **WHEN** the adapter reads the editor's advertised capabilities at initialize
- **THEN** it advertises only the editor-backed tools the editor actually supports

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
### Requirement: The CLI configures editors to launch the agent

The CLI SHALL provide `acp install [<editor>]` to register itself as an ACP agent. With no editor, it SHALL print the generic launch details (the command, `["acp"]`, and an optional `FLEETY_AGENT_URL`) that any ACP-capable editor uses. For a supported editor (`zed`), it SHALL merge an entry into that editor's config pointing at the current binary, preserving the editor's other settings and other agents, backing up the prior file, and SHALL NOT clobber a config it cannot safely parse (e.g. JSONC with comments) — printing the snippet to paste instead. Re-running SHALL overwrite an existing entry (an update, not a duplicate). `fleety update` SHALL re-point already-installed entries at the current binary without newly installing any.

#### Scenario: install configures a supported editor

- **WHEN** the user runs `fleety acp install zed`
- **THEN** an `agent_servers.Fleety` entry pointing at the current binary is written to Zed's settings, the editor's other settings are preserved, and the prior file is backed up

#### Scenario: re-run updates in place

- **WHEN** `fleety acp install zed` is run again (e.g. after the binary moved)
- **THEN** the existing Fleety entry is overwritten with the current binary path rather than duplicated

#### Scenario: an unparseable config is not clobbered

- **WHEN** the editor config cannot be parsed as plain JSON (it has comments)
- **THEN** the config is left unchanged and the entry to add is printed for manual use

<!-- @trace
source: acp-adapter
updated: 2026-07-04
code:
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/main.rs
-->
