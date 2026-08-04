# Using Fleety from an editor (ACP)

Fleety speaks the **Agent Client Protocol (ACP)**, so an ACP-capable editor
(Zed, and increasingly others) can drive the Fleety agent from its own agent
panel. This doc covers setup, updates, and troubleshooting.

## How it fits together

```
editor  ──ACP (stdio, JSON-RPC)──▶  `fleety acp`  ──WebSocket──▶  fleety-server
(Zed …)                             (CLI adapter,                 (the real agent:
                                     a thin bridge)                model, tools, memory)
```

- The editor launches **`fleety acp`** as a subprocess and talks JSON-RPC over
  stdin/stdout. It is the **same `fleety` binary** you use for `fleety ask` /
  `fleety chat` / `fleety config` — `acp` is just another subcommand.
- `fleety acp` **runs no agent itself**. It translates ACP ↔ the fleety-server
  WebSocket protocol. The real agent (model calls, tools, memory, audit) lives in
  **fleety-server**, which the adapter connects to per prompt.
- So two things must be in place: the editor launches the **adapter** (`fleety
  acp`), and the adapter can reach a running **server** (via `FLEETY_AGENT_URL`,
  default `ws://127.0.0.1:8787`). The server can be on the same machine or remote.

Wire details: messages are newline-delimited JSON-RPC (one object per line);
`initialize` / `session.new` / `session.prompt` / `session.cancel` map to the
server's conversation protocol; assistant output streams back as `session/update`
notifications. Verified end-to-end against Zed 1.9.

## Quick start (Zed)

1. **Install a stable `fleety` binary** — don't point the editor at a
   `target/debug` build, which gets overwritten/locked while you develop:

   ```bash
   cargo install --path crates/fleety-cli      # → ~/.cargo/bin/fleety(.exe)
   ```

2. **Register it in Zed** — run this from the binary you want Zed to launch:

   ```bash
   ~/.cargo/bin/fleety acp install zed --server ws://127.0.0.1:8787
   ```

   This writes an `agent_servers.Fleety` entry into Zed's `settings.json`, backing
   up the old file first.

3. **Start a server** for the adapter to reach (see [Using a real
   model](#using-a-real-model); with no model it echoes, which is fine for a first
   test):

   ```bash
   cargo run -p fleety-server
   ```

4. **Restart Zed**, open the agent panel, pick **Fleety**, and send a message.

## The `fleety acp install` command

ACP is a shared protocol, so the installer is not Zed-only:

| Command | What it does |
|---|---|
| `fleety acp install` | Prints the generic launch details (command, args, env) that **any** ACP editor uses. |
| `fleety acp install zed [--server <url>]` | Auto-configures Zed's `settings.json`. |
| `fleety acp install <other>` | No auto-config yet — prints the generic details to set up manually. |

**Zed auto-config specifics:**

- Writes `command` = the binary you ran the installer from (`current_exe`),
  `args` = `["acp"]`, and `env.FLEETY_AGENT_URL` when `--server` is given.
- `zed` and `--server <url>` may appear in either order. The endpoint must be a
  non-empty `ws://` or `wss://` URL without embedded credentials, terminal
  controls, or a URL fragment; validation failure returns non-zero before any
  settings file is touched.
- **Re-running updates it** (reports `Configured` on a fresh add vs `Updated` on a
  re-run) — handy after `cargo install` moves the binary.
- `FLEETY_TOKEN` is treated as bound to the configured transient endpoint. A
  same-endpoint re-run preserves it; changing or removing
  `FLEETY_AGENT_URL` clears it so an old bearer token cannot reach another
  Server or override the saved current profile.
- **Non-destructive**: other settings and other agent servers are preserved; the
  previous file is saved to `settings.json.bak`. Read, backup, or replacement
  failure returns non-zero. On Unix, the replacement, backup, and any recovery
  copy use owner-only `0600` permissions because they may contain
  `FLEETY_TOKEN`. Publication moves the exact file read by Fleety aside, compares
  those displaced bytes, then creates the destination only if it is still
  absent. If another process changes or recreates `settings.json`, Fleety
  preserves that path and reports the unique `.recovery` copy when needed.
  Any failure to restore displaced settings reports the recovery path instead
  of implying that the canonical file was preserved. If restoration succeeds
  but removing the recovery link fails, Fleety says the canonical settings are
  active and reports only the retained cleanup path.
  If publication succeeds but antivirus or filesystem policy prevents removing
  a private `.tmp` or `.recovery` file, Fleety reports the settings as active,
  warns with every retained path, and does not tell you to reapply the change.
  If publication itself fails and its private temp file also cannot be removed,
  the error reports that retained path for manual cleanup.
  Close Zed before installing or updating: no portable filesystem operation can
  stop a non-cooperating process from writing through a handle it opened before
  the update.
- **JSONC-safe**: if your `settings.json` has comments (so it can't be parsed as
  plain JSON), it is **not** clobbered — the snippet is printed for you to paste.
- The settings base (`HOME` or `APPDATA`) must be a non-empty absolute path.
  Otherwise the command prints the manual snippet and exits non-zero without
  writing relative to the working directory.
- `fleety update` refreshes an already-installed Fleety entry. An unreadable,
  invalid, or unwritable existing Zed settings file makes the update incomplete
  only when it contains that installed Fleety entry; unrelated JSONC settings
  without a Fleety entry are treated as a no-op and do not block the update.

Zed settings location: `%APPDATA%\Zed\settings.json` (Windows),
`~/.config/zed/settings.json` (macOS/Linux).

## Other editors (manual setup)

Any ACP-capable editor works — point its custom-ACP-agent command at the adapter.
Run `fleety acp install` to print the exact values, then use your editor's
settings. The Zed shape, as a reference:

```json
{
  "agent_servers": {
    "Fleety": {
      "type": "custom",
      "command": "/absolute/path/to/fleety",
      "args": ["acp"],
      "env": {}
    }
  }
}
```

Use an **absolute** path for `command` (editors don't always inherit your shell
`PATH`). With no endpoint environment override, ACP uses the saved current
profile and its credential, then the trusted local default when no profile is
configured. `FLEETY_AGENT_URL` is optional; when present, it is a transient
endpoint and never borrows a saved profile token. For an authenticated server,
either add `FLEETY_TOKEN` explicitly to the editor environment or omit
`FLEETY_AGENT_URL` so ACP uses the saved current profile.

The adapter bounds both WebSocket connection/upgrade and `Welcome` waits. It
sends no prompt, resume, or cancellation frame until the Server returns
`Welcome`. A saved profile must receive its exact non-empty pinned Server
identity in that `Welcome`; otherwise the adapter closes the connection without
forwarding editor content or accepting Server control.
An ACP turn completes only after `Done`. A disconnect, receive failure, or
failure to deliver a required editor-tool／approval reply fails the turn instead
of returning a misleading `end_turn` after partial output.

## Remote / shared server

Because the editor only runs a thin adapter, the actual agent can live elsewhere:
set `FLEETY_AGENT_URL` (in the editor's `env`, or via `fleety acp install --server
<url>`) to point at a server on another host. The same server can be shared by
Zed, `fleety chat`, scheduled runs, etc. Use `wss://` + TLS for anything beyond
loopback. `fleety acp install --server <url>` writes only the transient endpoint;
an authenticated endpoint also needs an explicit `FLEETY_TOKEN`, or a saved
current profile with the raw server override omitted.

## Using a real model

The server echoes by default (an offline stub), so it always boots. To use a real
model, configure it on the **server** (not the editor):

- Env: `FLEETY_MODEL_BASE_URL`, `FLEETY_MODEL` (+ optional `FLEETY_MODEL_KEY`,
  `FLEETY_MODEL_STREAM=1`). See [`env.md`](env.md#model-provider).
- Persisted: `fleety config set FLEETY_MODEL <name>` (+ base URL / key), or a
  `providers.toml` pool.
- ChatGPT subscription (no key): `auth = "oauth:codex"` — see
  [`env.md`](env.md#codex-chatgpt-oauth-sign-in-instead-of-an-api-key).

Changing the server's model takes effect on the **next thread** in the editor
(each prompt reconnects) — no editor restart needed.

## Updates

The editor's ACP config stores a **binary path**, not a version:

- **In-place CLI updates are transparent.** `fleety update` swaps the binary at
  the same path, and the editor relaunches that path — now the new binary. No
  config change needed.
- **`fleety update` self-heals installed configs.** As its last step it
  re-points any editor already set up for Fleety (currently Zed) at the current
  binary — covering a **changed path** (e.g. dev build → `cargo install`) or an
  evolved `acp` invocation. It never newly-installs, never touches an editor
  without a Fleety entry, and never clobbers a JSONC file.
- **The CLI does not auto-update when the server updates.** Only `fleetyd` (the
  daemon) converges to the server version on reconnect; the CLI is a client and
  stays compatible across versions. Run `fleety update` on the host to update it.

**Restart the editor after replacing the binary.** The editor keeps the agent
subprocess alive; a rebuild/update that swaps the binary needs a full editor
restart (or agent restart) before it launches the new one — opening a new thread
reuses the old process.

## Troubleshooting

| Symptom (in the editor) | Cause | Fix |
|---|---|---|
| `cannot connect to ws://127.0.0.1:8787` | The **server isn't running** (or `FLEETY_AGENT_URL` is wrong). | Start `fleety-server`, or fix the URL. |
| "Authentication Required" (older builds) | Same as above — a connection error was mis-coded as ACP auth. Fixed to show the real message. | Start the server. |
| "Failed to launch" / "send failed because receiver is gone" | The agent subprocess died or was replaced while the editor held it (common right after a rebuild/update). | **Fully restart the editor** so it spawns a fresh agent. |
| The reply doesn't render | An out-of-date `session/update` shape (fixed: it must be tagged `sessionUpdate` with a `content` block). | Update to a current `fleety`. |
| Nothing appears / editor can't parse | Something wrote to the adapter's **stdout** (which is protocol-only). Fleety sends all logs to stderr. | Report it — no non-JSON should reach stdout. |
