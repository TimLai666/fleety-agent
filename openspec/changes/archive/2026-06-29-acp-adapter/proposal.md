## Why

Editors like Zed speak the Agent Client Protocol (ACP) to drive a coding agent: the editor spawns the agent as a subprocess and talks JSON-RPC over stdio. Fleety already has a capable agent (the server: memory, skills, MCP, subagents) and, with session-workspace-cwd, the notion of working in a given directory. Adding an ACP front-end lets users drive Fleety as a coding agent from inside an ACP-capable editor, reusing that brain — no new agent logic, just a protocol adapter.

## What Changes

- A new `fleety acp` subcommand that runs an **ACP agent over stdio** (JSON-RPC 2.0): the editor launches it as a subprocess and exchanges ACP messages on stdin/stdout.
- It **bridges ACP to the existing fleety-server** rather than reimplementing the agent: ACP requests are translated to the server's WebSocket protocol and the server's responses are streamed back as ACP notifications. `agent-core` stays the host-free runtime; ACP is just another front-end alongside the WebSocket CLI.
- Mapping: `initialize` → capability negotiation; `session/new` → a new server conversation whose **working root is the ACP-provided `cwd`** (carried via the existing `OriginContext.cwd`, consumed by session-workspace-cwd); `session/load` → server `Resume`; `session/prompt` → a server `UserMessage`, with the server's `AssistantDelta`/`Assistant` streamed as ACP `session/update` notifications and the turn ended with a `session/prompt` stop reason; `session/cancel` → stop the in-flight turn.
- Permission: the server's `ApprovalRequested` is surfaced as ACP `session/request_permission`, and the user's choice is sent back as the server's `Approve`/`Deny`. Running under `FLEETY_POLICY=require_approval` makes non-read tools prompt the editor.
- stdio carries **only** the protocol — logs go to stderr so they don't corrupt the JSON-RPC stream.

## Non-Goals

- Not reimplementing the agent loop or tools in the adapter — it bridges to the server.
- Not delegating file/terminal I/O to the client via ACP `fs/*` / `terminal/*` for v1: the server (via session-workspace-cwd) runs tools rooted at the ACP cwd. Client-side `fs`/`terminal` delegation is a possible follow-up, not this change.
- Not changing the server's WebSocket protocol or `agent-core`.
- Not an ACP *client* (Fleety driving other agents) — this makes Fleety an ACP *agent*.

## Capabilities

### New Capabilities

- `acp-adapter`: a `fleety acp` stdio JSON-RPC front-end that makes Fleety an ACP agent — initialize/session(new,load,prompt,cancel) mapped to the fleety-server protocol, assistant output streamed as `session/update`, tool approvals surfaced as `session/request_permission`, the ACP session cwd feeding the workspace binding; logs on stderr, protocol on stdout.

### Modified Capabilities

(none — reuses the existing server protocol and session-workspace-cwd unchanged.)

## Impact

- Affected specs: new `acp-adapter`. Depends on the parked `session-workspace-cwd` change for cwd-rooted execution.
- Affected code:
  - New: crates/fleety-cli/src/acp.rs (the ACP stdio adapter: JSON-RPC framing, method handlers, server bridge, mapping tables), with the JSON-RPC and ACP message types and the pure mapping functions unit-tested
  - Modified: crates/fleety-cli/src/main.rs (add the `acp` subcommand and route stdout/stderr so only JSON-RPC is on stdout), docs/env.md (document `fleety acp`, how editors launch it, and that it bridges to the server URL)
  - Removed: none
