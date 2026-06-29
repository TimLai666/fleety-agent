## Why

`fleety acp` (acp-adapter) bridges an editor to fleety-server, but the agent's
file/terminal tools run on the **server**, not in the user's editor. The genuine
value of ACP is that the editor exposes its own fs/terminal to the agent, so
edits appear in the user's buffers (unsaved, with the editor's approval/diff UI)
and commands run in the editor's terminal on the editor's host. ACP defines these
client capabilities and conformant editors (e.g. Zed) already serve them, so the
work is entirely on our side — and because the editor's terminal can run arbitrary
commands on its host, nearly the whole workflow can run on the editor's machine
without co-locating the server or running a daemon there.

## What Changes

- **Editor-backed tools, named so the surface is unambiguous.** In an ACP
  conversation the agent gains a small set of clearly-named tools that act in the
  user's editor: editor_read_file / editor_write_file / editor_edit (the editor's
  text fs — buffer-aware) and editor_run (the editor's terminal — arbitrary
  commands on the editor's host: git, ls, grep, build, test, rm/mv, …). They
  appear only when this session has an editor that advertised the matching ACP
  capability. They never reroute or rename the server's own tools.
- **They target this conversation's editor.** An ACP connection is not a device;
  it is a per-conversation editor execution channel. Within a conversation there
  is exactly one editor, so the editor tools implicitly target it — addressed by a
  per-connection id (so multiple editors on one machine don't collide), while the
  connection still reports the device (host) it runs on so editor and disk stay on
  the same machine.
- **The agent is told to prefer the editor tools, and how the surfaces differ.**
  In an ACP conversation the system prompt instructs the agent to prefer editor_*
  for the user's work, and explains that editor edits live in the buffer (may be
  unsaved, need the user's approval) while disk reads (e.g. git via the terminal)
  won't see unsaved buffer edits until saved. Tool results carry the surface and
  saved status so the agent never mistakes a buffer edit for a persisted one.
- **The adapter becomes a bidirectional ACP agent.** `fleety acp` both answers the
  editor's requests (session/*) and calls the editor (fs/*, terminal/*), over a
  persistent connection that pumps tool round-trips while the turn streams. The
  server routes this conversation's editor tools to the connection (reusing the
  existing RunTool/ToolResult mechanism) and gains per-connection addressing.
- **Conformant editors need no changes** (Zed already serves ACP fs/terminal); we
  only delegate capabilities the editor advertises in initialize.

## Non-Goals

- Not a general cross-machine "route any session's tools to its origin device"
  framework for non-editor clients — the editor's terminal already covers host
  execution here; the general case stays a separate future change.
- Not rerouting or renaming the server's own workspace tools; they remain
  disk-backed and are the fallback when the editor lacks a capability.
- Not requiring a daemon or server co-location for an ACP session (the editor's
  terminal runs host commands).
- Not new protocol message types — reuse RunTool / ToolResult / ToolError; the
  per-connection id is server-internal.

## Capabilities

### New Capabilities

- `acp-editor-delegation`: editor-backed agent tools (named editor_read_file /
  editor_write_file / editor_edit over the editor's text fs, and editor_run over
  the editor's terminal) that execute in the user's editor on its host, target
  this conversation's editor via a per-connection id, are gated by the editor's
  advertised ACP capabilities, and are accompanied by a system-prompt preference
  + surface/saved labeling so the agent reasons correctly about buffer vs disk.

### Modified Capabilities

- `acp-adapter`: the adapter becomes a bidirectional ACP agent (answers session/*
  and calls the editor's fs/* and terminal/*) over a persistent connection;
  advertises the editor-backed tools and handles routed tool calls; reports the
  device (host) the editor session runs on.

## Impact

- Affected specs: new `acp-editor-delegation`; modified `acp-adapter`.
- Affected code:
  - Modified: crates/fleety-cli/src/acp.rs (bidirectional JSON-RPC: serve session/* and call fs/*+terminal/*; persistent concurrent bridge; advertise editor tools at Hello and report the host device; handle routed tool calls by translating to editor methods; gate by initialize clientCapabilities), crates/fleety-server/src/bridge.rs (per-connection addressing so multiple connections on one device can be targeted), crates/fleety-server/src/conn.rs (bind a conversation to its editor connection; register the conversation's editor-backed tools; inject the prefer-editor + surface system-prompt note for ACP sessions), crates/fleety-server/src/tools.rs (editor tool specs / routing proxies), docs/env.md (editor delegation: tools, preference, surfaces, identity, Zed needs no changes)
  - New: none required (reuse existing routing)
  - Removed: none
