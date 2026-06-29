## Context

`fleety acp` (acp-adapter, crates/fleety-cli/src/acp.rs) connects directly to
fleety-server over WebSocket and maps ACP session methods to the conversation
protocol. Today it sends `local_tools_json: None`, ignores `ServerMsg::RunTool`
(`_ => {}`), is a per-prompt ephemeral connection, and advertises only
`loadSession` in `initialize`. The agent's file/terminal tools therefore run on
the server. ACP defines client capabilities — `fs` (read/write text) and
`terminal` (arbitrary commands) — that conformant editors (Zed) already serve.
The server already has the routing primitives to send a tool call to a connected
client and await its result: `ServerMsg::RunTool` → `ClientMsg::ToolResult/ToolError`,
via the hub + pending map (bridge.rs), which the daemon uses. This change makes the
editor an execution surface by reusing that routing, with the editor as a
per-conversation channel rather than a device.

## Goals / Non-Goals

**Goals:**
- The agent can read/write/edit files in the user's editor (buffer-aware) and run
  commands in the editor's terminal, on the editor's host.
- The surface is unambiguous to the agent (named editor_* tools), and the agent is
  told to prefer them and how buffer vs disk differs.
- Works without co-locating the server or running a daemon (the editor's terminal
  is the host executor); the editor still reports its host so editor and disk are
  the same machine.
- Conformant editors need no changes; we only delegate advertised capabilities.

**Non-Goals:**
- No general cross-machine origin-routing framework for non-editor clients; no
  rerouting of the server's own tools; no daemon requirement; no new protocol
  message types.

## Decisions

### An ACP connection is a per-conversation editor channel, not a device

A device (daemon) is one-per-machine, persistent, disk-backed, addressed by
`device_id`. An ACP connection is one editor session — ephemeral, and there can be
many per machine. So it is modeled as a per-conversation execution channel, not a
device. Within a conversation there is exactly one editor (the connection that
serves it), so editor tools target it implicitly; the multiplicity ("many editors
per machine") is across conversations, each seeing only its own editor.

**Alternative:** treat the connection as a device with a `host#editor` id —
rejected (collides when one machine has several editors; pollutes the device
registry with ephemeral entries).

### Two-level identity: device_id (host) + a per-connection id

The ACP connection reports the device (host) it runs on via the existing Hello
`device_id` (so fleety knows which machine the editor is on, pairing editor and
disk on one host). To address a *specific* connection among several sharing a
`device_id`, the server assigns a unique per-connection id at connect and the
hub/routing addresses connections by it (today the hub keys by `device_id`, which
cannot distinguish multiple connections on one host).

**Alternative:** address by `device_id` only — rejected (multiple ACP connections
per host collide in the hub).

### Editor tools are a small named set over two ACP primitives

The editor exposes two primitives: `fs` (buffer-aware text read/write) and
`terminal` (arbitrary host commands). The agent sees named tools mapping to them:
`editor_read_file` / `editor_write_file` / `editor_edit` → fs; `editor_run` →
terminal (git, ls, grep, build, test, rm/mv/mkdir, …). The name carries the
surface; nothing reroutes or renames the server's tools. Because terminal runs
arbitrary commands on the editor's host, nearly the whole workflow runs there —
no co-location or daemon needed.

**Alternative:** a single `editor_exec(tool, args)` shell like `device_exec` —
rejected (nested/opaque args; worse for the model; loses per-tool schemas). The
editor subset is small and known, so named tools are clearer.

**Alternative:** per-named-tool for every shell op (editor_git_status, …) —
rejected (`editor_run` over the terminal covers them all; don't enumerate shell).

### Writes/edits prefer fs (buffer); queries/commands/destructive go via terminal

`editor_write_file` / `editor_edit` use fs so the change appears in the user's
buffer with the editor's diff/approval and its own undo — the core value. Reads
for the agent's view can use fs (buffer) too. git, search, listing, build, run,
and destructive ops (rm/mv) go via `editor_run` (terminal). The server's
structured tools (backups/rollback, precise edit, path guards) remain the fallback
when the editor lacks a capability; raw terminal trades those for the editor's/
git's own safety nets, which is unavoidable when the server is remote from the
editor's disk.

**Alternative:** force all writes through the terminal — rejected (loses buffer
integration, the whole point; `echo >`/`sed` are unsafe vs structured edits).

### Capability gating from initialize; tools appear only when usable

The adapter reads the editor's advertised `clientCapabilities` from the ACP
`initialize` request and only advertises the editor_* tools the editor actually
supports (no terminal capability → no `editor_run`, agent falls back to disk
`run_command`). The editor_* tools appear in the agent's registry only for
conversations that have such an editor.

**Alternative:** always advertise editor_* and fail at call time — rejected
(misleads the agent; gate up front).

### The agent is told to prefer editor tools and how surfaces differ

For an ACP conversation the system prompt instructs the agent to prefer editor_*
for the user's work, and explains: editor edits live in the buffer (may be unsaved,
need the user's approval); disk reads (git/ls via terminal) won't reflect unsaved
buffer edits until saved, so ask the user to save when disk must reflect them.
Editor tool results carry `surface` and `saved` so the agent never treats a buffer
edit as persisted.

**Alternative:** describe surfaces but don't instruct preference — rejected (the
user explicitly wants the agent to prefer the editor tools; passive description
leaves the choice to chance).

### The adapter becomes a bidirectional, persistent ACP agent

`fleety acp` must both answer the editor's requests (session/*) and call the
editor (fs/*, terminal/*) — bidirectional JSON-RPC over the one stdio. Its
WebSocket to the server becomes a persistent session connection that, while a
prompt streams, also receives `RunTool` frames, translates each to the
corresponding editor request, awaits the editor's response, and replies
`ToolResult`/`ToolError` — i.e. it pumps tool round-trips concurrently with
assistant streaming (today it is per-prompt and drops RunTool).

**Alternative:** keep the ephemeral per-prompt bridge — rejected (can't service
tool calls mid-turn).

## Implementation Contract

**Behavior:** In an ACP conversation whose editor advertises fs (and optionally
terminal), the agent is offered editor_read_file/editor_write_file/editor_edit
(and editor_run when terminal is advertised), is instructed to prefer them, and
when it calls them the operation runs in the user's editor on its host: edits
appear in the user's buffer with the editor's approval/diff; editor_run executes
in the editor's terminal and returns output/exit status. Each editor tool result
reports its surface and saved state. The editor is addressed as this
conversation's connection (per-connection id); the connection reports its host
device_id. Capabilities the editor doesn't advertise are not offered (agent falls
back to the server's disk tools). Conformant editors need no changes. Nothing
panics; a failed editor call returns an actionable error, not a crash.

**Interfaces / data shapes:**
- ACP adapter: read `clientCapabilities` from `initialize`; advertise the
  supported editor_* tool specs at Hello (local_tools_json); on `RunTool` for an
  editor_* tool, translate to the ACP method (fs/read_text_file,
  fs/write_text_file, terminal/create+output+wait) and reply ToolResult/ToolError;
  report the host `device_id`.
- Server routing: assign a per-connection id; hub/pending address a specific
  connection; bind a conversation to its serving editor connection; register the
  conversation's editor_* tools (routed to that connection).
- System prompt: an ACP-session addendum (prefer editor_*; buffer vs disk; unsaved
  caveat) injected only when the conversation has an editor.
- Tool result: editor_* results include `surface` (editor-buffer | editor-terminal)
  and, for writes, `saved` (bool/unknown).
- approval → ACP `session/request_permission`.

**Failure modes:** editor lacks a capability → that editor_* tool is not offered;
agent uses the disk fallback. Editor returns an error/declines permission → an
actionable ToolError to the agent (not a crash). Connection drops mid-turn →
in-flight editor calls fail with a clear error; the turn degrades, no panic.
Multiple editors on one host → distinct per-connection ids, no cross-talk.
Unsaved-vs-disk mismatch → surfaced via labeling + the system prompt; the agent
asks the user to save. Protocol method-name mismatch with the editor → verified at
build against the ACP spec; unknown editor responses are handled, not panicked.

**Acceptance criteria:**
- Pure mapping tests: editor_* tool call → correct ACP request shape
  (fs/read_text_file, fs/write_text_file, terminal/*); capability gating picks the
  right tool set from a sample `clientCapabilities`.
- Dispatch tests (injectable editor side): a routed editor_write_file produces an
  fs write request and maps the editor's response to a ToolResult with
  surface/saved; an unsupported capability yields no such tool.
- Server routing test: with two simulated connections sharing a device_id, a tool
  routed to conversation A reaches connection A, not B (per-connection addressing).
- System-prompt test: an ACP conversation includes the prefer-editor addendum;
  a non-ACP conversation does not.
- agent-core stays host-free; fmt + clippy --workspace -D warnings green; full
  test suite green.
- A live editor (Zed) session — real buffer edits, terminal, approval — is
  environment-dependent and manual-verify.

**Scope boundaries:**
- In: editor_* named tools over fs + terminal, capability gating, per-conversation
  editor channel + per-connection addressing + host device_id reporting,
  bidirectional persistent ACP bridge, prefer-editor system prompt + surface/saved
  labeling, approval mapping, docs, the testable mapping/dispatch/routing/prompt
  pieces.
- Out: general cross-machine origin routing for non-editor clients, rerouting
  server tools, daemon requirement, new protocol message types, a live-editor
  automated test.

## Risks / Trade-offs

- [bidirectional JSON-RPC + concurrency] → the adapter must serve and call over one
  stdio while streaming; isolate this as the bridge's core and test mapping/
  dispatch with an injectable editor side; the live path is manual-verify.
- [buffer vs disk incoherence] → labeling + a prescriptive system prompt (prefer
  editor_*, save before disk-reads); all on one machine (the editor's host), so no
  cross-machine confusion.
- [losing structured-tool safety on terminal ops] → writes/edits stay on fs
  (buffer + editor undo); destructive/query ops via terminal accept editor/git as
  the safety net; server structured tools remain the fallback.
- [per-connection addressing touches shared routing] → additive (assign an id,
  address by it); the daemon path (device_id) keeps working.
- [editor capability variance] → gate strictly on advertised clientCapabilities;
  unsupported → disk fallback; no hard dependency on any editor feature.
- [ACP method-name accuracy] → verify exact method/capability names against the
  ACP spec at implementation; handle unknown responses gracefully.
- [identity] → the connection reports its host device_id (pairs editor + disk on
  one machine) and gets a unique per-connection id (distinguishes editors); both
  are required and captured as a requirement.
