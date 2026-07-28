# Fleety Agent — Tool Surface (canonical)

This is the source of truth for the tools the Fleety Agent (the LLM) may call.
`prompts/protocol.md` describes how to use them in prose; this file fixes the
**names, typed inputs, returns, and risk class**. When a name changes, change
it here first, then sync `prompts/protocol.md` and the runtime. The runtime
still exposes each tool's real JSON Schema at call time — that schema wins
over this doc for argument shape.

Last reviewed against `crates/` on 2026-06-28.

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
    git, insyra, the `browser_*` (CDP) tools, the `computer_*` (desktop) tools.
    To route one of these to a specific device, wrap it with
    `device_exec(device="…", tool="read_file", args={…})`; call it by its bare
    name and you hit the server's workspace.
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
implementations register on `fleety-server` (relative paths against
`FLEETY_WORKSPACE`) and inside every `fleetyd` (against `FLEETY_DEVICE_ROOT`).
Call by bare name → hits the server; wrap in `device_exec(device="laptop",
tool="read_file", args={…})` → hits the laptop's filesystem instead.

**Filesystem scope.** By default (the `full_access` posture) these tools are
**not sandboxed to the root** — absolute paths and paths outside the root work
(read or audited + rollback-backed write), since the root is just the base for
*relative* paths. A sensitive-path guard still refuses **mutations** of critical
paths (SSH keys/config, `/etc/shadow`, `/dev`, Windows system dirs, …) with an
actionable "do this deliberately / confirm with the user" error; reads are
unrestricted. Set `FLEETY_FS_SCOPE=workspace` to re-confine everything to the
root (the old `..`/absolute/symlink-tight sandbox).

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `read_file` | Read a UTF-8 text file. Returns raw `content` + line-numbered `numbered` + `line_count`; slice with `start_line`/`end_line`. | `path`, `start_line?`, `end_line?` | read |
| `list_dir` | List a directory. | `path?` (default `.`) | read |
| `search_files` | ripgrep over the workspace (respects `.gitignore`, skips binaries). | `query`, `path?`, `max_results?` | read |
| `write_file` | Write a whole file (overwrite). Returns `backup` + unified `diff`. | `path`, `content` | mutate |
| `read_file_bytes` | Read **any** file (binary-safe) as base64 — returns `content_b64` + `sha256` + `bytes`. The byte-exact counterpart to `read_file` (which is UTF-8 only); the read half of cross-device transfer. Refuses over `FLEETY_TRANSFER_MAX_BYTES` (64 MiB default). | `path` | read |
| `write_file_bytes` | Write **any** file (binary-safe) from base64 — decodes, size-checks, sensitive-path-guards, backs up if it existed, returns `sha256` + `bytes` + `backup?`. Overwrites by default (like `write_file`); pass `overwrite: false` to refuse an existing target. The write half of cross-device transfer. | `path`, `content_b64`, `overwrite?` | mutate |
| `edit_file` | Precise edit — substring mode (`old`→`new`, unique) or line-range mode (`start_line`..`end_line`→`new`). Returns `backup` + `diff` + `applied` (numbered post-edit region). | `path`, `new`, and `old?` or `start_line?`/`end_line?` | mutate |
| `delete_file` | Delete a file (backup first). | `path` | mutate |
| `move_file` | Move / rename (backs up destination if it exists). | `from`, `to` | mutate |
| `make_dir` | Create a directory (and any missing parents). | `path` | mutate |
| `rollback` | Restore a file from a `backup_id` returned by a prior mutation. | `backup_id` | mutate |
| `run_command` | Run one command in the workspace; returns `stdout`/`stderr`/`exit_code`. Pass `track: [paths]` to get a unified before/after diff of files it touched. The critical-command guard rejects irreversible shapes (wipe / mkfs / dd / `rm -rf /` / etc.). | `command`, `cwd?`, `track?` | mutate, or **critical** when the critical-command guard matches |

> Read before you rely; re-read before each edit (line numbers / content
> shift). Mutations all back up first, so `rollback` is always available.

## Git (read-only)

**Runs on:** any device. Same dual registration as the workspace tools above.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `git_status` | Working-tree status. | — | read |
| `git_diff` | Unstaged working-tree diff; **includes untracked new files**. | — | read |
| `git_log` | Recent commit log. | `limit?` | read |
| `git_show` | Show a commit / ref (default `HEAD`). | `ref?` | read |

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
| `ssh_exec` | Run a command on a remote host over SSH. The target is built defensively (no option injection in `host`); batch-mode only (no interactive password). | `host`, `command`, `user?`, `port?`, `identity?` (private-key path) | mutate (critical for irreversible commands) |

## Browser (CDP)

**Runs on:** any device. The browser tools live in the shared `fleety-tools`
crate, so they're registered on `fleety-server` and on every `fleetyd` —
`device_exec(device="laptop", tool="browser_screenshot")` drives the **laptop's**
browser, a bare `browser_*` call drives the server's. The persistent-session map
is process-wide, so a `browser_open` on a device lives in that device's daemon
and later session-scoped calls routed there reuse the same connection.

Drives a real Chrome over the DevTools Protocol. The endpoint defaults to
`http://127.0.0.1:9222` (override per-call with `chrome`, or globally with
`FLEETY_CHROME_URL`). When the local endpoint is down, Chrome is
**auto-provisioned** — see below.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `browser_open` | Open a persistent CDP session; returns `session`. Auto-provisions a local Chrome if none is running. | `chrome?` | mutate |
| `browser_navigate` | Navigate (a `session`, or a fresh connection). | `url`, `session?`, `chrome?` | mutate |
| `browser_eval` | Evaluate JS in the page; returns the value. | `expression`, `session?`, `chrome?` | mutate (critical when the JS posts / sends / deletes) |
| `browser_screenshot` | PNG screenshot (base64). The low-impact way to observe a device's screen. | `session?`, `chrome?` | read |
| `browser_close` | Close a session. | `session` | mutate |

**Chrome auto-provisioning.** Before connecting to a local endpoint that isn't
up, the runtime ensures one (on whichever device the tool runs):

1. Endpoint already reachable → use it.
2. Local but down → find an installed Chrome/Chromium and launch it headless on
   the port (`--remote-debugging-port`, isolated profile).
3. None installed + auto-install on → OS package manager (winget / brew /
   apt|snap), then a **chrome-for-testing** `chrome-headless-shell` download
   unpacked into a managed cache dir.

A managed (downloaded) Chrome is kept current by a 24h background loop on the
server; a system / package-manager Chrome self-updates. Remote endpoints (a
non-loopback `FLEETY_CHROME_URL` / `chrome=`) are never provisioned. Knobs:
`FLEETY_CHROME_AUTO_INSTALL=0` (detect+launch only), `FLEETY_CHROME_BIN`
(force a binary), `FLEETY_CHROME_DIR` (managed-download cache) — see
[`docs/env.md`](env.md).

## Computer-use (native desktop control)

**Runs on:** any device. Native (`enigo` input + `xcap` capture) in the shared
`fleety-tools` crate, so `device_exec(device="laptop", tool="computer_click")`
drives the **laptop's** desktop, a bare call drives the server's. Needs a real
display session — headless hosts and Linux/Wayland (synthetic input restricted)
return an actionable error instead of acting.

This is the **most intrusive** interface — `policy.md` ranks it last (prefer a
dedicated API/MCP > `browser_*` CDP > computer-use): it takes over the user's
own mouse/keyboard, so warn before driving a device the user is actively using,
and destructive desktop actions are `critical`. Screenshots are exempt
(low-impact).

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `computer_screenshot` | Capture the desktop as a base64 PNG. | `monitor?` | read |
| `computer_move` | Move the cursor to absolute pixels. | `x`, `y` | mutate |
| `computer_click` | Click `left`/`right`/`middle` (optionally move to `x`,`y` first). | `button?`, `x?`, `y?` | mutate |
| `computer_type` | Type a string at the current focus. | `text` | mutate |
| `computer_key` | Press a key (char or named: enter/tab/esc/arrows/f1-f12…) with optional modifiers (ctrl/alt/shift/meta). | `key`, `modifiers?` | mutate |
| `computer_scroll` | Scroll the wheel (`vertical`/`horizontal`). | `amount`, `axis?` | mutate |

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
> `<root>/.insyra`. Load the built-in `fleety-use-insyra-dsl` skill for the full
> `.isr` DSL command reference. Resolved via `FLEETY_INSYRA_BIN` → beside the
> exe → `PATH`. fleetyd auto-provisions it on `install` / `update`.

## Video (scene-aware extraction)

**Runs on:** any device. Registered on `fleety-server` and every `fleetyd` (like
the workspace tools), so `device_exec(device="phone", tool="video_extract", …)`
extracts on the device that holds the video; a bare call runs on the server.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `video_extract` | Turn a video (URL — YouTube/Instagram/TikTok/… — or a workspace file) into **scene-aware** keyframes (real visual changes, not fixed-interval samples) plus an optional transcript and a manifest, via the `claude-real-video` (`crv`) engine. Returns `{out_dir, manifest_path, manifest, frames[], frame_count, transcript_path?, notes[]}` — read frames with `read_file_bytes` (JPEGs) and the transcript with `read_file`. | `source`, `out?`, `scene?`, `fps_floor?`, `max_frames?`, `lang?`, `transcribe?` | mutate |

> Downloads via yt-dlp and runs ffmpeg, so it egresses + writes files. Transcription
> needs Whisper and is opt-in (`FLEETY_VIDEO_WHISPER=on`); without it, frames are
> still extracted and a note says transcription was skipped. `crv` is auto-installed
> (pip/pipx), `ffmpeg` via the platform package manager; override with
> `FLEETY_CRV_BIN` / `FLEETY_FFMPEG_BIN`, bound by `FLEETY_VIDEO_TIMEOUT_SECS`
> (default 900s). Load the built-in `fleety-real-video` skill for the workflow.

## Agent memory

**Runs on:** server only. These tools operate on the agent's **core** memory
files only (`ME.md` / `USER.md` / `TODO.md` / `TOOLS.md`) under
`{FLEETY_AGENT_HOME}/fleet/`. A device's `NOTES.md` is read via `device_show`,
not here (there is no `device` argument).

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `memory_read` | Read a core memory file. Returns raw `content` + line-numbered `numbered` + `line_count`; slice with `start_line`/`end_line`. | `file`, `start_line?`, `end_line?` | read |
| `memory_write` | Write a core memory file whole — `mode` `replace` (default) or `append`. | `file`, `content`, `mode?` | mutate |
| `memory_edit` | Precise edit — substring mode (`old`→`new`, unique unless `replace_all`) or line-range mode (`start_line`..`end_line`→`new`). Returns the post-edit `applied` region. | `file`, `new`, and `old?` or `start_line?`/`end_line?`, `replace_all?` | mutate |

> The core files `ME.md` / `USER.md` / `TODO.md` are auto-injected into the
> system prompt every turn. Use `memory_edit` for surgical updates and
> `memory_write` to create or wholesale-replace — memory you never update rots
> into lies.

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
| `device_exec` | Run a tool on a connected device by id (routes a `RunTool` frame to that daemon, awaits the reply). **Strict**-checks `tool` against the device's advertised list when the device advertised; legacy devices that didn't advertise are not strict-checked. Byte tools (`read_file_bytes` / `write_file_bytes`) route through here too. | `device`, `tool`, `args?`, `handle?` | mutate |
| `transfer_file` | Copy one file between two endpoints — an endpoint is a device id, or `"server"`/empty for the server workspace. Reads the source (`read_file_bytes`, locally or via `device_exec`), writes the dest (`write_file_bytes`), and **verifies the sha256 end-to-end** — a mismatch is a corruption error, not a success. Returns `{ok, bytes, sha256, from, to}`. Device↔device, device↔server, either direction. | `from`, `from_path`, `to`, `to_path`, `overwrite?` (default true) | mutate |
| `pair_create` | Mint a short-lived pairing code (10 min) so a new device can enroll. | — | mutate |
| `site_list` | List known sites. | — | read |
| `site_show` | A site plus the devices located there. | `site` | read |
| `site_set` | Create/update a site. | `id`, `name?`, `description?` | mutate |
| `site_delete` | Delete a site (device records unchanged). | `id` | mutate |
| `device_set_presence_opt_in` | Enable/disable presence tracking for a device (server-side, off by default). | `device`, `enabled` | mutate |
| `device_set_home_site` | Set a device's usual place (baseline for presence/departure). Must be a registered site. | `device`, `home_site` | mutate |
| `site_bind_fingerprint` | Bind a device's currently reported network fingerprint to a site, so future reports place it there. | `device`, `site` | mutate |
| `presence_show` | Devices at a site + a probabilistic "is a person present" signal with confidence, reasons, and a caveat. | `site` | read |
| `device_presence` | A device's current site, home site, and a probabilistic departure signal. | `device` | read |

> Presence tools are opt-in and probabilistic. Nothing is recorded for a device
> until it opts in (`device_set_presence_opt_in`) **and** its daemon runs with
> `FLEETY_PRESENCE=on`. Every presence answer carries a confidence — reachability
> is not presence.

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

## Subagents (delegation)

**Runs on:** server only (these orchestration tools). The parent agent delegates
a task to a **subagent** — a nested agent loop with the same tools MINUS these
orchestration tools, so a subagent cannot spawn its own subagents (one-level
cap). A subagent keeps every other tool, including `device_exec`, so it can
still act on other devices. The mechanism is a generic agent-core capability —
see [subagent-framework.md](subagent-framework.md) for the `SubagentHost`
contract. It runs on the `main` or `cheap` model tier (see
`FLEETY_CHEAP_MODEL_*` in [env.md](env.md)); the tier changes only the provider,
never the policy/gate/audit. A background subagent reports back by proactively
waking a coordinator turn when it finishes. See
`crates/fleety-server/src/subagent.rs`.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `spawn_subagent` | Delegate a task. `mode` `spawn` (fresh context + briefing) / `fork` (inherits the current conversation). `model` `main`/`cheap`. `run_in_background` returns a `task_id` immediately and wakes a turn on completion; otherwise awaits and returns the `output`. `isolation` `none`/`worktree` (a dedicated git worktree; errors if the workspace isn't a git repo). `allowed_tools` whitelists tools under require-approval. | `prompt`, `mode?`, `model?`, `run_in_background?`, `isolation?`, `allowed_tools?`, `name?` | mutate |
| `send_subagent_message` | Continue an existing (finished, not running) subagent, preserving its context. Addressable by `task_id` **or** the worker's `name`. | `task_id`, `prompt` | mutate |
| `stop_subagent` | Stop a subagent (by `task_id` or `name`); a background task is aborted, state becomes `stopped`. | `task_id` | mutate |
| `subagent_status` | Report a subagent's state and (when finished) its output. By `task_id` or `name`. | `task_id` | read |
| `subagent_list` | List your team — every subagent you spawned with its `task_id`, `name`, and `state`. The roster for coordinating an **agent team** (lead routes between named workers via `send_subagent_message`). | — | read |
| `run_workflow` | **Dynamic workflow.** Run a JS script that deterministically orchestrates your own subagents — `agent({prompt,...})` runs one subagent, plus `parallel`/`pipeline`/`phase`/`log`. The script body uses top-level `await` and `return`s its result. For when the orchestration shape is dynamic and worth pinning down as code. Runs on an embedded engine (`agent-workflow` crate); subagents it launches are leaves (no nesting). | `script` | mutate |

## Goal (drive a request to completion)

**Runs on:** server only (these are top-level tools). An always-on mechanism for
finishing a whole request in one go instead of stopping halfway to ask "shall I
continue?". When you set a goal, the server's drive-to-goal loop keeps
re-engaging you each turn (injecting a continuation nudge that names the goal and
the pending steps) until you signal a terminal state — `complete_goal` (done) or
`ask_user` (a question only the user can answer) — or it hits the auto-continue
cap (`FLEETY_GOAL_MAX_CONTINUES`, see [env.md](env.md)). Intermediate
continuation turns are silent: only the terminal turn produces the user-facing
reply (and, in voice mode, the spoken summary); progress still streams as deltas.
Like the orchestration tools, these are registered ONLY at the top level — a
subagent's registry omits them, so a subagent cannot touch its parent's goal. The
generic state + tools live in `agent-core` (`crates/agent-core/src/goal.rs`); the
loop and emission/voice gating live in `crates/fleety-server/src/conn.rs`.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `set_goal` | Record the goal you are pursuing (derived from the user's request + context) plus an optional checklist. Engaging this is what turns on the drive-to-goal loop. Call again to revise the goal/plan (clears any terminal signal). | `goal`, `steps?` | read |
| `complete_step` | Mark one checklist step done (matched by its text). Returns the updated checklist; errors listing current steps if the text matches none. | `step` | read |
| `goal_status` | Report the current goal and which checklist steps are done or pending. | — | read |
| `complete_goal` | **Terminal.** Declare the goal achieved — the loop stops and your reply goes to the user. Call only when the whole goal is genuinely done. | `summary?` | read |
| `ask_user` | **Terminal.** Ask a question you genuinely cannot proceed without — the loop stops and the question goes to the user. Use sparingly; not for "shall I continue?". | `question` | read |

## Reasoning effort (self-tuning)

**Runs on:** server only (top-level tool). Registered ONLY for the main agent —
a subagent's registry omits it (a subagent's effort is fixed by its parent at
spawn). Lives in `crates/fleety-server/src/effort.rs`; the connection loop
re-reads the chosen effort before **every** turn it drives — including each
goal-continuation turn — and selects an effort-variant provider (see
`per-task-effort`).

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `set_effort` | Set YOUR OWN reasoning effort. Does **not** change the step you are on — it applies from your **next** turn onward (including the next goal-continuation turn of the current request) and persists until you change it again. `auto` clears your manual choice and hands control back to the runtime's automatic, difficulty-based selection. | `level` (`low` / `medium` / `high` / `auto`) | read |

> When you have not pinned a level and `FLEETY_AUTO_EFFORT` is on (the default),
> the runtime classifies each incoming message's difficulty on the economy tier
> and starts that turn in the right gear. So raising effort mid-task mainly helps
> the *continuation* turns of the current request; for a one-shot turn, the
> auto-classifier is what gets the first inference right. See
> [`docs/env.md`](env.md).

## Skills

**Runs on:** server only. Skills live in `{FLEETY_AGENT_HOME}/skills/` across
three tiers that merge by name with **installed > authored > builtin**
precedence:

- **builtin** — shipped in the server binary, re-seeded every boot, read-only.
- **authored** — skills the agent writes for itself from experience
  (Hermes-style). The agent owns these and manages them **autonomously**, no
  user consent needed.
- **installed** — user-chosen packs. The agent installs/removes these **only
  at the user's request**; they shadow a builtin/authored skill of the same
  name.

`list_skills` tags each entry with its `source`.

A skill is a **directory** — it may hold `SKILL.md` plus scripts / reference
files — so editing is file-level. The tier of a skill decides what may mutate
it; a write to a skill that doesn't exist yet lands in **authored**.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `list_skills` | List skills; each carries `source: "builtin" \| "authored" \| "installed"` and its `path`. | — | read |
| `use_skill` | Load a skill's `SKILL.md`; follow it for the current task. Also returns the skill's `path` (its directory) — run a bundled `scripts/<x>` tool with `run_command` on that absolute path (wrap in `device_exec` for another device). | `name` | read |
| `skill_validate` | Check a SKILL.md against the Agent Skills format — pass `content` (a draft) or `name` (existing). Returns `ok` + `issues` (error/warning): missing YAML frontmatter, missing/invalid `name` (≤64, lowercase/digits/hyphens, no `anthropic`/`claude`) or `description` (non-empty, ≤1024), over-long body, or a `name` not matching the directory. | one of `name`/`content` | read |
| `skill_install` | Install/replace a **user** skill into the installed tier (only when the user asks). Body from `content`, `from_url` (public hosts, SSRF-guarded), or `from_path` (local SKILL.md / whole skill dir). | `name`, one of `content`/`from_url`/`from_path` | mutate |
| `skill_remove` | Remove a whole skill. Authored: free. Installed: only at user request. Builtin: refused (shadow it instead). | `name` | mutate |
| `skill_list_files` | List the files inside a skill + its tier. | `name` | read |
| `skill_read_file` | Read a file in a skill (default `SKILL.md`); numbered + `line_count`, optional `start_line`/`end_line`. | `name`, `file?`, `start_line?`, `end_line?` | read |
| `skill_write_file` | Create/overwrite a file in a skill (SKILL.md, a script, a reference). New skill → **authored**. This is how multi-file skills are authored. | `name`, `file?`, `content` | mutate |
| `skill_edit_file` | Precise edit of a skill file — substring or line-range mode; returns the post-edit `applied` region. | `name`, `file?`, `old`/`new` or `start_line`/`end_line`/`new` | mutate |
| `skill_delete_file` | Delete a file in a skill (not `SKILL.md` — use `skill_remove` for the pack). | `name`, `file` | mutate |

> **Tier rules (enforced).** **builtin** skills are read-only — every mutating
> tool refuses them (to customise one, `skill_install` or author a skill of the
> same name; it shadows the builtin). **authored** skills the agent owns and
> edits autonomously. **installed** skills should be created/edited/removed
> **only at the user's request**.

## External MCP

**Runs on:** server only. `mcp_call` spawns the configured stdio MCP process on
the server's host; the MCP server's own tools then run with the server's
network / filesystem. User-installed MCP servers shadow built-ins of the same
name; the spawn is single-shot (initialize → `tools/call` → kill) with a 30 s
timeout.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `mcp_list` | List configured MCP servers; each entry carries `source: "builtin" \| "installed"`. Built-in: `ddgs` (web search). | — | read |
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

**Install & auto-update.** ddgs is a Python package; the runtime installs and
keeps it updated automatically — a `fleety-server` upgrade refreshes the
bundled MCP through whichever channel applies:

- `scripts/install-server.sh` runs `pipx install ddgs[mcp]` on first run, and
  `pipx upgrade ddgs` (fallback `pip install -U --user`) when re-run after a
  release — so the one-liner that updates the server also refreshes the
  bundled MCP
- The official Docker image (`Dockerfile`) bakes `ddgs[mcp]` into the runtime
  layer; `docker compose up -d --build` pulls the latest PyPI version
- At server boot: missing → install inline; already installed → background
  `pipx upgrade` (fallback `pip -U --user`) so a binary swap picks up latest
- On long-running servers, a 24h background loop keeps the built-in MCPs at
  their upstream release without operator action (configurable via
  `FLEETY_DDGS_UPGRADE_SECS`)
- `FLEETY_DDGS_AUTO_INSTALL=0` disables both the install and the upgrade
  paths (air-gapped / hermetic deployments)

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
| `wiki_search` | Exact substring search over the vault (case-insensitive). | `query` | read |
| `wiki_semantic_search` | Search by **meaning** — local EmbeddingGemma vectors, cosine-ranked. Finds related notes even with different wording. Returns top notes with `score` + snippet. | `query`, `top_k?` | read |
| `wiki_read` | Read a page; returns raw `content` + line-numbered `numbered` + `line_count` (slice with `start_line`/`end_line`). | `path`, `start_line?`, `end_line?` | read |
| `wiki_list` | List pages. | — | read |
| `wiki_write` | Create or overwrite a page (markdown; use frontmatter + `[[wikilinks]]`). | `path`, `content` | mutate |

**Semantic search engine.** `wiki_semantic_search` runs a local **EmbeddingGemma
300M** model (`onnx-community/embeddinggemma-300m-ONNX`, Q8) in-process via
fastembed/ONNX on CPU — no external service. The first call (or the boot-time
background warm) downloads the model once (~300MB) into `{FLEETY_AGENT_HOME}/
models/`, then runs offline. Vectors are indexed in an **HNSW** graph (`hnsw_rs`)
for fast approximate nearest-neighbour search; the metadata (chunk text +
vectors + content hash) is the source of truth at `{vault}/.index/
embeddings.json`, and the graph is rebuilt from it whenever a note changes (HNSW
can't delete, so a rebuild also handles removals). The index stays current
automatically: each search re-embeds any note whose hash changed and drops
deleted ones, and `wiki_write` re-embeds the edited note immediately. The graph
lives in memory while serving (rebuilt from the persisted metadata, never lost).
`FLEETY_WIKI_EMBED=0` disables it (no download; the tool returns an actionable
error pointing at `wiki_search`). See [`docs/env.md`](env.md).

## CLI-only surfaces (not invokable by the agent)

These are not tools the LLM picks — they're operator commands. Listed here
so the surface is complete.

| CLI | Purpose |
|---|---|
| `fleety init <url>` | Save the agent URL into `~/.fleety/config.json`. |
| `fleety pair <code>` | Enroll this device with a code minted by `pair_create` somewhere else. |
| `fleety ask "..."` | One-shot conversation. Multimodal: `--image PATH`, `--audio PATH`, `--video PATH`, `--file PATH` (read once, base64-encoded, attached). |
| `fleety chat` | Interactive multi-pane TUI. **Ctrl+V** pastes a clipboard image (re-encoded to PNG) or a single-line file path as an attachment; **Ctrl+X** clears staged attachments. See [TUI input](#tui-input) for the composer and mouse keys. |
| `fleety conversations resume <conv> [after_seq]` | Replay a conversation. |
| `fleety status` | Version, uptime, connected devices, sidecar health (insyra binary path / missing). |
| `fleety audit list [N]` / `fleety audit show <i>` | Browse the audit log; `5m ago` relative time. |
| `fleety rollback list` / `fleety rollback apply <id>` | Inspect and restore backups. |

### TUI input

The Chat composer is [`fleety-textarea`](../crates/fleety-textarea), a vendored
copy of grok-build's editor, so it answers the usual readline keys as well as
Fleety's own.

| Keys | Does |
|---|---|
| Enter / Alt+Enter / Ctrl+J | Send / newline / newline |
| Ctrl+Z, Ctrl+Y | Undo, redo |
| Ctrl+W, Ctrl+K, Ctrl+U | Kill previous word, to end of line, to start of line |
| Ctrl+Y | Yank back the last kill |
| Alt+←/→, Ctrl+←/→ | Move by word |
| Ctrl+V, Ctrl+X | Paste attachment, clear staged attachments (Fleety's, not the composer's cut) |

Long lines wrap and grow the box rather than scrolling sideways.

Assistant replies render through [`fleety-markdown`](../crates/fleety-markdown),
also vendored from grok-build: full CommonMark, with fenced code blocks
syntax-highlighted by language, plus tables, task lists and links. Markdown
markers stay on screen but dimmed, so a reply still reads as what the model
wrote. A bare newline stays a line break — Fleety turns off the CommonMark rule
that would fold it into the previous line.

A ` ```mermaid ` fence is drawn as a diagram rather than shown as source:
`graph` / `flowchart`, `sequenceDiagram` and `stateDiagram` become Unicode line
art. Any other diagram type falls back to its raw source in a framed box. This
is pure text — no graphics protocol and no external process — so it renders the
same in every terminal.

Chat opens with the Fleety wordmark, the version, the Server it reached, and the
model — written into the scrollback once, so it scrolls away as the conversation
grows instead of occupying the viewport forever.

Chat does not take over the screen. It keeps a small viewport at the bottom and
writes the conversation above it as ordinary terminal output, so the exchange
becomes part of your scrollback: scroll it with the terminal's own scrollbar,
select and copy it with a plain drag, and it is still there after you quit.
Fleety takes no mouse input and no longer scrolls the conversation itself.

While a reply is streaming, the part whose markdown has closed is written out as
it settles; the part still being written stays in the viewport until it does.

## Built-in MCP / sidecars

| Sidecar | Owns | Provisioned by |
|---|---|---|
| `fleety-insyra` (Go) | `insyra_exec` over NDJSON | `fleetyd install` / `fleetyd update` |
| `fleety-use-insyra-dsl` (built-in skill, shipped in-binary) | DSL reference for `insyra_exec` | `builtin_skills::seed` at server boot |
| `insyra` (built-in skill, shipped in-binary) | upstream Insyra skill, vendored verbatim | `builtin_skills::seed` at server boot |
| `claude-real-video` (`crv`, Python CLI) | the `video_extract` engine (scene-aware frames + transcript) | server dep auto-install (pip/pipx); ffmpeg via OS package manager |
| `fleety-real-video` (built-in skill, shipped in-binary) | how to use `video_extract` | `builtin_skills::seed` at server boot |

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
`project_*`, `workspace_*` aliases, and built-in `browser` skill action
vocabulary. Those are still planned but not in the runtime today. (Desktop
control shipped natively as the `computer_*` tools — see the Computer-use
section above; `conversation_list` / `conversation_search` shipped as the
conversation-recall tools.) See `docs/spec-v0.md` for scope and
`docs/roadmap.md` for next-up implementation plans. When they land, they
belong in this file in the corresponding section.
