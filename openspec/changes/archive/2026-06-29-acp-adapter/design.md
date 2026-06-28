## Context

ACP (agentclientprotocol.com) lets an editor drive a coding agent: the editor (client) spawns the agent as a subprocess and they speak JSON-RPC 2.0 over stdio. Client→agent methods: `initialize`, `authenticate`, `session/new`, `session/load`, `session/prompt`, `session/cancel` (notification). Agent→client: `session/update` notifications (streaming progress), `session/request_permission` (tool authorization), and `fs/*` / `terminal/*` for client-mediated I/O. Paths are absolute; line numbers 1-based.

Fleety's agent already exists in the server (memory, skills, MCP, subagents) and `agent-core` is a host-free runtime designed to sit behind different front-ends — the WebSocket CLI is one. This change adds an ACP front-end that bridges to the server rather than duplicating the agent, and reuses session-workspace-cwd so the ACP-provided working directory becomes the conversation's root.

## Goals / Non-Goals

**Goals:**
- `fleety acp`: a stdio JSON-RPC ACP agent the editor spawns; only JSON-RPC on stdout, logs on stderr.
- Bridge ACP lifecycle to the existing server protocol; stream assistant output as `session/update`; surface approvals as `session/request_permission`.
- Use the ACP session `cwd` as the conversation's working root (via session-workspace-cwd / `OriginContext.cwd`).
- Pure JSON-RPC framing + ACP↔server mapping functions are unit-testable; the live editor session is manual-verify.

**Non-Goals:**
- No agent/tool reimplementation in the adapter (bridge to server).
- No client-mediated `fs/*` / `terminal/*` in v1 (server runs tools rooted at cwd); a possible follow-up.
- No server-protocol or `agent-core` change.
- Not an ACP client.

## Decisions

### `fleety acp` is a stdio JSON-RPC front-end that bridges to the server

`fleety acp` runs an event loop reading newline/Content-Length-framed JSON-RPC from stdin and writing responses/notifications to stdout. It opens a WebSocket to the fleety-server (same discovery as the existing CLI: `FLEETY_AGENT_URL` / mDNS) and translates between the two protocols. `agent-core` and the server are unchanged; ACP is a peer of the WebSocket CLI front-end. **stdout carries only JSON-RPC**; tracing/logs are redirected to stderr so they can't corrupt the stream.

**Alternative:** run `agent-core` in-process in the CLI (self-contained ACP agent) — rejected for v1 (would duplicate provider/tool/memory/skill wiring that already lives in the server; bridging reuses the brain). Left open as a future "serverless ACP" mode.

### Method mapping (ACP ↔ fleety-server)

- `initialize` → respond with protocol version + agent capabilities (advertise `loadSession`; do **not** advertise client `fs`/`terminal` since v1 doesn't use them).
- `authenticate` → satisfied by Fleety's existing token/pairing (token from env/CLI config); a no-op when the server doesn't require auth.
- `session/new { cwd, mcpServers }` → open a conversation; carry `cwd` as `OriginContext.cwd` so session-workspace-cwd roots the conversation there; return an ACP session id mapped to the server `conversation_id`.
- `session/load { sessionId }` → server `Resume` for the mapped conversation, replaying history as `session/update` notifications.
- `session/prompt { prompt }` → server `UserMessage`; the server's `AssistantDelta` stream becomes `session/update` (assistant message chunks) and tool activity becomes `session/update` tool-call entries; the turn's end (`Done`) becomes the `session/prompt` response carrying a stop reason.
- `session/cancel` (notification) → signal the in-flight turn to stop (close/cancel the current server request); no response.

**Alternative:** map prompts to one-shot `ask` without streaming — rejected (editors expect incremental `session/update`).

### Permissions map to `session/request_permission`

When the server emits `ApprovalRequested { tool, summary, risk }` (i.e. under `FLEETY_POLICY=require_approval`), the adapter sends ACP `session/request_permission` with the tool + summary and, on the user's choice, replies to the server with `Approve`/`Deny`. Allow/deny (and any allow-always) outcomes map to proceed/deny; a cancelled permission cancels the turn. With full-access policy (the default), no permission prompts are raised — same as today.

**Alternative:** auto-approve everything — rejected (defeats the editor's safety UX; respect the server policy).

### Workspace rooting reuses session-workspace-cwd

The ACP `cwd` is the user's project directory. Rather than a second rooting mechanism, the adapter passes it as `OriginContext.cwd` on the conversation's messages, so the server's session-workspace-cwd resolution roots the conversation (and routes tools to the originating device) exactly as for the WebSocket CLI. This change therefore **depends on session-workspace-cwd**; absent it, the adapter still works but tools run on the server's default workspace.

**Alternative:** a dedicated ACP workspace path on the wire — rejected (duplicates session-workspace-cwd; the cwd channel already exists).

### Logs to stderr, protocol to stdout

Because the editor parses stdout as JSON-RPC, the adapter installs logging on stderr only for the `acp` subcommand (the rest of the CLI is unchanged). Malformed inbound JSON-RPC yields a JSON-RPC error response, never a panic or a stray stdout write.

## Implementation Contract

**Behavior:** An ACP-capable editor configured to launch `fleety acp` can open a session in a project directory, send a prompt, and see streamed assistant output and tool activity; tool approvals appear as editor permission prompts when the server policy requires them; the conversation operates in the editor's directory. Resuming a session continues the same conversation. Cancelling stops the turn. Only JSON-RPC is written to stdout; everything else goes to stderr. Malformed input produces a JSON-RPC error, not a crash.

**Interfaces / data shapes:**
- JSON-RPC framing: request/response/notification structs (id, method, params, result, error) with encode/decode; pure and tested.
- ACP method handlers for `initialize`, `authenticate`, `session/new`, `session/load`, `session/prompt`, `session/cancel`; an outbound emitter for `session/update` and `session/request_permission`.
- Pure mapping functions: server `AssistantDelta`/`Assistant` → `session/update` payload; `ApprovalRequested` → `session/request_permission` params; `Done` → stop reason; ACP `cwd` → `OriginContext`. These are unit-tested without a live socket or editor.
- A bridge that owns the server WebSocket (reusing the CLI's connect/discovery) and a session-id ↔ conversation-id map.

**Failure modes:** malformed JSON-RPC in → JSON-RPC error out (no panic, no stdout noise). Server unreachable → `initialize`/`session/new` returns a JSON-RPC error with an actionable message. Server disconnect mid-turn → emit a terminal `session/update` + error stop reason; the editor can retry. Unknown ACP method → JSON-RPC method-not-found. Permission request with no user response before cancel → treat as denied. Never write non-protocol bytes to stdout; never panic the loop.

**Acceptance criteria:**
- Unit tests for JSON-RPC encode/decode (request/response/notification/error round-trip; malformed → error).
- Unit tests for each pure mapping: AssistantDelta→session/update, ApprovalRequested→request_permission, Done→stop reason, cwd→OriginContext, session/new→conversation open params.
- A dispatch test: each ACP method name routes to its handler; unknown method → method-not-found.
- A bridge test with an injectable server transport (no real socket): a prompt produces the expected ordered session/update notifications and a final stop reason; a server ApprovalRequested produces a request_permission and the reply maps to Approve/Deny.
- Content review: docs/env.md documents `fleety acp`, editor launch config, server URL/auth, and the v1 non-use of client fs/terminal.
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.
- Live editor session (e.g. Zed launching `fleety acp`) is environment-dependent and manual-verify.

**Scope boundaries:**
- In: `fleety acp` stdio JSON-RPC loop, ACP method handlers, server bridge + session map, pure mapping/framing functions + their tests, stdout/stderr discipline, docs.
- Out: client `fs`/`terminal` delegation, in-process serverless mode, ACP-client role, server-protocol/agent-core changes, editor-specific config files (documented, not shipped).

## Risks / Trade-offs

- [stdout contamination breaks JSON-RPC] → logs forced to stderr for `acp`; framing tested; no stray prints in the loop.
- [depends on session-workspace-cwd for cwd rooting] → degrades gracefully (server default workspace) if that change isn't applied; note the dependency.
- [bridging adds a hop vs in-process] → accepted for v1 (reuses the full server brain); in-process mode left as a future option.
- [ACP spec evolution / version negotiation] → `initialize` negotiates the version and advertises only implemented capabilities; unimplemented methods return method-not-found.
- [v1 doesn't use client fs/terminal, so editor diffs/buffers aren't surfaced] → acceptable MVP (server edits disk at cwd); client-mediated I/O is a documented follow-up.
- [permission UX depends on server policy] → default full-access raises no prompts (same as today); require_approval surfaces them in the editor.
