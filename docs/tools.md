# Fleety Agent — Tool Surface (canonical)

This is the source of truth for the tools the Fleety Agent (the LLM) may call.
`prompts/protocol.md` describes how to use them in prose; this file fixes the
**names, typed inputs, returns, and risk class**. When a name changes, change
it here first, then sync `prompts/protocol.md` and the runtime. The runtime
still exposes each tool's real JSON Schema at call time — that schema wins
over this doc for argument shape.

Last reviewed against `crates/` on 2026-06-24.

## Conventions

- **Where each tool runs.** Each section below carries a **Runs on:** marker.
  The three values are:
  - **server only** — the tool's code only lives in `fleety-server`. The agent
    can't reach another device's filesystem / network this way. Examples: web
    egress (`fetch_url`, `http_request`, `ws_call`, `sse_stream`), wiki,
    schedules, the device registry.
  - **any device** — the tool lives in the shared `fleety-tools` crate, so it's
    registered in **both** the server's registry (operating on
    `FLEETY_WORKSPACE`) and every fleetyd's local registry (operating on
    `FLEETY_DEVICE_ROOT`). Examples: `read_file`, `write_file`, `run_command`,
    git, insyra. To route one of these to a specific device, wrap it with
    `device_exec(device="…", tool="read_file", args={…})`; call it by its
    bare name and you hit the server's workspace.
  - **server-only routing** — the tool exists on the server but its job is to
    coordinate other devices (e.g. `device_exec` itself, `pair_create`).
- **Targeting.** Tools that operate on a workspace ("workspace tools") run on
  the **server's** `FLEETY_WORKSPACE`. Tools that operate on a device's local
  filesystem (`device_exec`, `ssh_exec`) explicitly take a `device` argument.
- **Risk class** (drives the access policy in `prompts/policy.md`):
  - `read` — no state change. Executes directly under any policy.
  - `mutate` — changes state. Under `full_access` executes directly but is
    **audited + rollback-backed**. Under stricter policy returns
    `approval_required`.
  - `critical` — irreversible / no rollback path. **Always requires explicit
    user confirmation**, even under `full_access`.
- **Return envelope.** Tool results are JSON objects whose shape is described
  per-tool. Errors come back as actionable messages with hints to retry / fix
  arguments / acquire permissions, not as bare strings.
- **Backups.** Every mutating workspace tool first writes the prior content to
  `{home}/fleet/backups/<uuid>/` and returns a `backup.id` you can pass to
  `rollback`. Diffs work on any device, not just git repos.
- **Approvals.** Under `FLEETY_POLICY=require_approval`, mutating/critical
  tools surface an `ApprovalRequested` over the WebSocket before running;
  on `Deny` the agent gets a synthetic `tool_denied` result and continues.
  See `prompts/policy.md` and `crates/agent-core/src/approval.rs`.

---

## Workspace tools

**Runs on:** any device. Lives in the shared `fleety-tools` crate, so the same
implementations register on `fleety-server` (against `FLEETY_WORKSPACE`) and
inside every `fleetyd` (against `FLEETY_DEVICE_ROOT`). Call by bare name → hits
the server's workspace. Wrap in `device_exec(device="laptop", tool="read_file",
args={…})` → hits the laptop's filesystem instead.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `read_file` | Read a UTF-8 text file. | `path` | read |
| `list_dir` | List a directory. | `path?` (default `.`) | read |
| `search_files` | ripgrep over the workspace (respects `.gitignore`, skips binaries). | `query`, `path?`, `max_results?` | read |
| `write_file` | Write a whole file (overwrite). Returns `backup` + unified `diff`. | `path`, `content` | mutate |
| `edit_file` | Replace an exact, unique substring. Returns `backup` + `diff`. | `path`, `old`, `new` | mutate |
| `delete_file` | Delete a file (backup first). | `path` | mutate |
| `move_file` | Move / rename (backs up destination if it exists). | `from`, `to` | mutate |
| `make_dir` | Create a directory (and any missing parents). | `path` | mutate |
| `rollback` | Restore a file from a `backup_id` returned by a prior mutation. | `backup_id` | mutate |
| `run_command` | Run one command in the workspace; returns `stdout`/`stderr`/`exit_code`. Pass `track: [paths]` to get a unified before/after diff of files it touched. The critical-command guard rejects irreversible shapes (wipe / mkfs / dd / `rm -rf /` / etc.). | `command`, `track?`, `timeout_secs?` | mutate, or **critical** when the critical-command guard matches |

> Read before you rely; re-read before each edit (line numbers / content
> shift). Mutations all back up first, so `rollback` is always available.

## Git (read-only)

**Runs on:** any device. Same dual registration as the workspace tools above.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `git_status` | Working-tree status. | — | read |
| `git_diff` | Working-tree (or staged) diff; **includes untracked new files**. | `staged?`, `path?` | read |
| `git_log` | Recent commit log. | `limit?` | read |
| `git_show` | Show a commit / ref. | `ref` | read |

## Web / HTTP / WebSocket / SSE

**Runs on:** server only. Egresses from `fleety-server`'s network, not the
target device. SSRF-guarded: only `http`/`https` (or `ws`/`wss` for `ws_call`),
loopback / RFC1918 / IPv6 ULA / link-local refused unless
`FLEETY_ALLOW_PRIVATE_NET=1`.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `fetch_url` | Simple HTTP GET; returns `status`, `content_type`, `body`, `truncated`. | `url`, `max_bytes?` | read |
| `http_request` | Any of GET/POST/PUT/PATCH/DELETE/HEAD. Supports per-call `timeout_secs`, `follow_redirects` (default 0), `verify_tls`, `headers`, `body` **xor** `multipart { fields[], files[] }`, `retry { max, backoff_ms, on_status }`, `cookie_jar` (persistent named jar), `stream_to_file` (workspace path → returns size + sha256 instead of body). | `method`, `url`, plus the knobs above | mutate |
| `ws_call` | One-shot WebSocket: handshake → send N text frames → receive up to `max_frames` (deadlined) → close. Shares the cookie jar with HTTP via the `http://` equivalent origin. | `url`, `send?`, `max_frames?`, `timeout_secs?`, `headers?`, `cookie_jar?` | mutate |
| `sse_stream` | Subscribe to a `text/event-stream` endpoint. Returns up to `max_events` events inline, or `stream_to_file` writes each event as one JSONL record. | `url`, `max_events?`, `timeout_secs?`, `headers?`, `cookie_jar?`, `verify_tls?`, `stream_to_file?` | read |

> Multipart files are read from the workspace (path-escape guard). Cookies
> persist under `{FLEETY_AGENT_HOME}/fleet/cookies/<name>.json` and survive
> across calls — pass the same `cookie_jar: "session1"` to keep an OAuth /
> session-bound API logged in.

## SSH

**Runs on:** server only. The SSH session originates from `fleety-server`'s
host using its own keychain / config / network egress.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `ssh_exec` | Run a command on a remote host over SSH. The target is built defensively (no option injection in `host`); batch-mode only (no interactive password). | `host`, `command`, `port?`, `user?`, `identity_file?`, `timeout_secs?` | mutate (critical for irreversible commands) |

## Browser (CDP)

**Runs on:** server only. Drives a real Chrome via the DevTools Protocol;
`fleety-server` opens the CDP connection. Set `FLEETY_CHROME_URL` to the
DevTools endpoint (e.g. `http://localhost:9222`). Each `browser_open` returns
a `session` id; subsequent ops take that id.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `browser_open` | Open a new browser tab; returns `session`. | `url?` | mutate |
| `browser_navigate` | Navigate an existing session. | `session`, `url` | mutate |
| `browser_eval` | Evaluate JS in the page; returns the value. | `session`, `expression` | mutate (critical when the JS posts / sends / deletes) |
| `browser_screenshot` | PNG screenshot. | `session` | read |
| `browser_close` | Close a session. | `session` | mutate |

## Insyra (data analysis DSL)

**Runs on:** any device. Like the workspace tools, registered on both
`fleety-server` and every `fleetyd`; sessions live in whichever process spawned
the sidecar. Route to a specific device via `device_exec(device="…",
tool="insyra_exec", args={…})`.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `insyra_exec` | Run the Insyra `.isr` DSL — load CSV/Parquet/Excel/SQL, transform, stats, plot. Stateful per `session`; `save <var> <file>` writes results into the workspace; `reset: true` clears the session. | `command` \| `script`, `session?`, `reset?` | mutate |

> Backed by the `fleety-insyra` Go sidecar that wraps Insyra's `engine/dsl`,
> kept alive per session, with named environments persisted under
> `<root>/.insyra`. Load the built-in `use-insyra-cli` skill for the full
> `.isr` DSL command reference. Resolved via `FLEETY_INSYRA_BIN` → beside the
> exe → `PATH`. fleetyd auto-provisions it on `install` / `update`.

## Agent memory

**Runs on:** server only. Memory lives in `{FLEETY_AGENT_HOME}/fleet/` —
agent's core (`ME.md` / `USER.md` / `TODO.md` / `TOOLS.md`) and per-device
records (`NOTES.md`). The `device?` argument selects which **device's** notes
file to read/write; it doesn't route the tool to that device — the file
itself is always read from the server's storage.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `memory_read` | Read a memory file. With `device`: that device's memory (`NOTES.md`); without: an agent-level core file (`ME.md` / `USER.md` / `TODO.md` / `TOOLS.md`). | `device?`, `file` | read |
| `memory_write` | Update a memory file (replace). | `device?`, `file`, `content` | mutate |

> The core files `ME.md` / `USER.md` / `TODO.md` are auto-injected into the
> system prompt every turn. Use `memory_write` to record what you learn —
> memory you never update rots into lies.

## Audit log

**Runs on:** server only. The full audit lives in
`{FLEETY_AGENT_HOME}/fleet/devices/<id>/history.jsonl` with `ts_secs` per line;
CLI surfaces are `fleety audit list` / `fleety audit show` and `fleety
rollback list` / `fleety rollback apply`.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `history_list` | List recent audit entries for this device. | `limit?` (default 20) | read |

## Devices & sites

**Runs on:** server only (router). The device registry lives on the server;
`device_exec` is how the agent **reaches** another device's local tools, but
`device_exec` itself runs on the server (it dispatches a `RunTool` frame over
the WebSocket bridge). `pair_create` is the enrollment seam the agent uses to
onboard a new device.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `device_list` | List registered devices and their records. | — | read |
| `device_show` | One device's record + NOTES + **advertised tools** (what `device_exec` can call there). | `device` | read |
| `device_set_site` | Assign a device to a site (or `away` / `unknown`). | `device`, `site` | mutate |
| `device_set_mobility` | Mark `stationary` / `mobile` / `unknown`. | `device`, `mobility` | mutate |
| `device_exec` | Run a tool on a connected device by id (routes a `RunTool` frame to that daemon, awaits the reply). **Strict**-checks `tool` against the device's advertised list when the device advertised; legacy devices that didn't advertise are not strict-checked. | `device`, `tool`, `args?`, `handle?` | mutate |
| `pair_create` | Mint a short-lived pairing code (10 min) so a new device can enroll. | — | mutate |
| `site_list` | List known sites. | — | read |
| `site_show` | A site plus the devices located there. | `site` | read |
| `site_set` | Create/update a site. | `id`, `name?`, `description?` | mutate |
| `site_delete` | Delete a site (device records unchanged). | `id` | mutate |

> Device-scoped handles: anything `device_exec` hands back (a session, a PID,
> a handle) is bound to its device. The runtime rejects using a handle against
> a different device. Re-issue against the owning device, or get a fresh
> handle on the one you actually meant.

## Schedules (self-managed cron)

**Runs on:** server only. The scheduler loop lives in the server and persists
records under `{FLEETY_AGENT_HOME}/fleet/schedules/`. A fired job spawns an
agent run with the stored prompt under `RequireApproval` + `MandateGate` (only
`allowed_tools` are usable). See `crates/fleety-server/src/scheduler.rs`.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `schedule_create` | Create a schedule. `trigger` is one of `at:<unix>` / `every:<30s\|5m\|2h\|1d\|n>` / a 5-field cron (`cron:<expr>` prefix optional). `tz` is an IANA name (default UTC); cron is evaluated in that zone. `allowed_tools` is the strict mandate enforced at fire time — anything else is denied. | `trigger`, `prompt`, `tz?`, `mandate?`, `allowed_tools?` | mutate |
| `schedule_list` | List the agent's schedules with `next_fire_secs` annotated per record. | — | read |
| `schedule_delete` | Remove a schedule by id. | `id` | mutate |

## Skills

**Runs on:** server only. Skills live in `{FLEETY_AGENT_HOME}/skills/` —
built-in (shipped in the server binary, refreshed every boot) plus
user-installed (preserved across updates; shadow built-ins by name).

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `list_skills` | List available skills (`SKILL.md` packs) — built-in + user-installed. | — | read |
| `use_skill` | Load a skill's instructions; follow them for the current task. | `name` | read |

## External MCP

**Runs on:** server only. `mcp_call` spawns the configured stdio MCP process on
the server's host; the MCP server's own tools then run with the server's
network / filesystem. User-installed MCP servers shadow built-ins of the same
name; the spawn is single-shot (initialize → `tools/call` → kill) with a 30 s
timeout.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `mcp_list` | List configured MCP servers; each entry carries `source: "builtin" \| "installed"`. | — | read |
| `mcp_add` | Add (or replace) a **user-installed** server. Shadows a built-in of the same name. | `name`, `command`, `args?` | mutate |
| `mcp_remove` | Remove a user-installed server. Built-in servers cannot be removed (override by `mcp_add`-ing the same name). | `name` | mutate |
| `mcp_call` | Call a tool on a configured MCP server. | `server`, `tool`, `arguments?` | mutate |

### Built-in: `ddgs` — web search

Server seeds `ddgs` into `{FLEETY_AGENT_HOME}/mcp/builtin.json` at every boot —
agents see it in `mcp_list` immediately and use it via `mcp_call(server="ddgs",
tool=…)`. This is Fleety's only **general web** search; the workspace's
`search_files` is ripgrep over local files, the wiki's `wiki_search` is the
agent's Obsidian vault. Neither reaches the public internet.

| ddgs tool (called via `mcp_call`) | Purpose |
|---|---|
| `search_text` | Web text search; metasearch aggregator (DuckDuckGo / Bing / Yandex / Brave / Mojeek / etc.). |
| `search_images` | Image search. |
| `search_news` | News search. |
| `search_videos` | Video search. |
| `search_books` | Book search. |
| `extract_content` | Extract page content from a URL (markdown-ified). |

**Install.** ddgs is a Python package; the runtime installs it automatically:

- `scripts/install-server.sh` runs `pipx install ddgs[mcp]` (then `pip --user`)
  right after dropping the binary
- The official Docker image (`Dockerfile`) bakes `ddgs[mcp]` into the runtime
  layer (`pipx`-installed under `/usr/local/bin`)
- At server boot, if `ddgs` still isn't on PATH the runtime tries the install
  itself (pipx → pip --user → `python -m pip --user`). Set
  `FLEETY_DDGS_AUTO_INSTALL=0` to opt out (e.g. air-gapped hosts).

Manual fallback if all of the above missed: `pip install -U 'ddgs[mcp]'`.

`FLEETY_DDGS_BIN` overrides the resolved path; `FLEETY_DDGS_ARGS` overrides the
spawn args (defaults to `["mcp"]`; pass `["mcp","-pr","socks5h://…"]` for
ddgs's proxy mode). See [`docs/env.md`](env.md).

## Knowledge wiki

**Runs on:** server only. The agent's long-term Obsidian vault under
`{FLEETY_AGENT_HOME}/wiki/`. Separate from per-device memory and
per-conversation history.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `wiki_search` | Search the vault. | `query` | read |
| `wiki_read` | Read a page. | `path` | read |
| `wiki_list` | List pages. | — | read |
| `wiki_write` | Create or update a page (frontmatter + `[[wikilinks]]` preserved). | `path`, `content`, `frontmatter?` | mutate |

## CLI-only surfaces (not invokable by the agent)

These are not tools the LLM picks — they're operator commands. Listed here
so the surface is complete.

| CLI | Purpose |
|---|---|
| `fleety init <url>` | Save the agent URL into `~/.fleety/config.json`. |
| `fleety pair <code>` | Enroll this device with a code minted by `pair_create` somewhere else. |
| `fleety ask "..."` | One-shot conversation. Multimodal: `--image PATH`, `--audio PATH`, `--video PATH`, `--file PATH` (read once, base64-encoded, attached). |
| `fleety tui` | Interactive multi-pane TUI. **Ctrl+V** pastes a clipboard image (re-encoded to PNG) or a single-line file path as an attachment; **Ctrl+X** clears staged attachments. |
| `fleety resume <conv> [after_seq]` | Replay a conversation. |
| `fleety status` | Version, uptime, connected devices, sidecar health (insyra binary path / missing). |
| `fleety audit list [N]` / `fleety audit show <i>` | Browse the audit log; `5m ago` relative time. |
| `fleety rollback list` / `fleety rollback apply <id>` | Inspect and restore backups. |

## Built-in MCP / sidecars

| Sidecar | Owns | Provisioned by |
|---|---|---|
| `fleety-insyra` (Go) | `insyra_exec` over NDJSON | `fleetyd install` / `fleetyd update` |
| `use-insyra-cli` (built-in skill, shipped in-binary) | DSL reference for `insyra_exec` | `builtin_skills::seed` at server boot |

## Risk class → policy (summary)

| Class | `full_access` (default) | `require_approval` |
|---|---|---|
| `read` | direct | direct |
| `mutate` | direct, audited + rollback-backed | `ApprovalRequested` → re-call after `Approve` |
| `critical` | **blocked pending explicit user confirmation** | blocked pending explicit user confirmation |

The same gate fires for scheduled (unattended) turns via `MandateGate`: only
`allowed_tools` named at schedule creation can run; everything else is denied.
Denials are recorded in the audit as `tool_denied` with the real tool name —
see `fleety audit list`.

## Planned (not yet shipped)

The earlier draft of this file listed `harness`, `capability_probe`,
`conversation_list/search/read`, `project_*`, `workspace_*` aliases, the
`computer-use-mcp` plane, and built-in `browser` skill action vocabulary.
Those are still planned but not in the runtime today; see `docs/spec-v0.md`
for scope and `docs/roadmap.md` for next-up implementation plans. When they
land, they belong in this file in the corresponding section.
