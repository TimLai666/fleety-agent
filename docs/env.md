# Fleety environment variables

The complete reference for every `FLEETY_*` variable the runtime reads.
Grouped by which binary cares about it. Anything unset uses the default.

## Server (`fleety-server`)

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_ADDR` | `127.0.0.1:8787` | WebSocket listen address. Bind `0.0.0.0:8787` to expose on the LAN. |
| `FLEETY_AGENT_HOME` | `$HOME/.fleety/agent` | Durable store root: conversations, history, backups, skills, MCP config, schedules, wiki. |
| `FLEETY_WORKSPACE` | cwd | Base directory the workspace tools (`read_file`/`write_file`/etc.) resolve **relative** paths against — the *fallback* workspace root (see below). |

**Per-conversation workspace root.** Each conversation roots its tools by this
precedence: the originating CLI's working directory (`origin.cwd`, sent on every
message) **when the CLI is on the same host as the server** → else `FLEETY_WORKSPACE`
→ else the server's cwd. So opening the CLI inside a project directory on the box
running the server makes Fleety a coding agent in that directory. The binding is
resolved once per conversation and reused (and persisted across resume). `cwd` is
treated as untrusted input (validated; the `FLEETY_FS_SCOPE` posture and the
sensitive-path guard still apply). When the CLI runs on a *different* device, the
server keeps the fallback root and records the originating device on the binding
(running tools on that remote device at its cwd is a planned follow-up).

| `FLEETY_FS_SCOPE` | (unset → `full`) | `full` (default): the structured file tools may read/write anywhere on the device (absolute paths allowed; still audited + rollback-backed; a sensitive-path guard refuses SSH keys/`/etc/shadow`/`/dev`/Windows system dirs/etc.). `workspace`: re-confine every path to the workspace/device root (`..`/absolute/symlink-tight sandbox). Set on `fleetyd` too for its `FLEETY_DEVICE_ROOT`. |
| `FLEETY_POLICY` | `full_access` | `require_approval` gates every non-read tool through the approval flow. |
| `FLEETY_REQUIRE_AUTH` | `0` | Set to `1` to require a valid token / pairing code on every `Hello`. |
| `FLEETY_TOKEN` | (unset) | Bootstrap admin token. Use it once to pair the first device. |
| `FLEETY_SCHED_TICK` | `60` | Seconds between scheduler fire-loop ticks. |
| `FLEETY_SYSTEM_PROMPT` | (unset → full) | `minimal` drops the embedded behavioural docs (protocol/rules/memory/policy) from the system message, leaving only core memory (ME/USER/TODO) — for token-lean / debugging runs. |
| `FLEETY_SUBAGENT_MAX_CONCURRENT` | `4` | Max background subagents running at once per connection. A spawn past the cap errors rather than queueing. Clamped to a floor of 1. |
| `FLEETY_GOAL_MAX_CONTINUES` | `8` | Max automatic goal continuations per user message. When the agent sets a goal (`set_goal`) and a turn ends without `complete_goal`/`ask_user`, the drive-to-goal loop re-engages it; this caps how many extra turns it may run before stopping and reporting the goal may be incomplete. Clamped to a floor of 1. |
| `FLEETY_SKILL_REFLECT_MIN_STEPS` | `5` | Tool-step threshold above which a completed user message triggers one learning-loop reflection turn (the agent is prompted to save a reusable procedure as an authored skill and durable facts to memory/wiki). Below the threshold nothing runs; `0` disables reflection entirely. |

## CLI (`fleety`) — voice input (speech-to-text)

Read by the `fleety voice` terminal. Microphone capture is built in (via `cpal`); transcription runs a local engine you point these at (default whisper.cpp). Anything missing → the CLI falls back to OS dictation (Windows) or typing, never crashing.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_STT_CMD` | (unset → whisper.cpp) | Transcription command template. `{wav}` and `{model}` are substituted. Unset uses the whisper.cpp default `whisper-cli -m <model> -f <wav> -nt` (which needs `FLEETY_STT_MODEL`). |
| `FLEETY_STT_MODEL` | (unset) | Path to the transcription model (e.g. a whisper.cpp `ggml-*.bin`). Required when using the default command. |
| `FLEETY_STT_SECONDS` | `5` | Seconds of microphone audio to record per spoken utterance. |

## Model provider

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_MODEL_BASE_URL` | (unset → echo) | OpenAI-compatible `/v1` root (OpenAI, OpenRouter, vLLM, Ollama, LM Studio, …). |
| `FLEETY_MODEL` | (unset → echo) | Model name to request. |
| `FLEETY_MODEL_KEY` | (unset) | Bearer token, when the endpoint needs one. |
| `FLEETY_MODEL_STREAM` | `0` | Set to `1` to use the SSE streaming endpoint (token-by-token TUI display). |

### Economy model (optional second tier for subagents)

A subagent can run on the **main** model or a cheaper **economy** model, chosen
per spawn. The cheap tier is its own independent provider (different
provider/model is fine). When unset, the cheap tier aliases the main model, so
selecting `cheap` always works. Same shape as `FLEETY_MODEL_*`: the model name
is the bare var, base URL / key / stream are suffixed.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_CHEAP_MODEL` | (unset → cheap = main) | Economy model name. A second provider is built only when this **and** `FLEETY_CHEAP_MODEL_BASE_URL` are set. |
| `FLEETY_CHEAP_MODEL_BASE_URL` | (unset) | OpenAI-compatible `/v1` root for the economy model. |
| `FLEETY_CHEAP_MODEL_KEY` | (unset) | Bearer token for the economy endpoint. |
| `FLEETY_CHEAP_MODEL_STREAM` | `0` | `1` to stream the economy model. |

## Retention / GC (server background loop)

Six-hour periodic sweep that keeps audit + backup surfaces bounded.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_GC_DISABLED` | (unset) | Set anything to skip the loop entirely. |
| `FLEETY_GC_INTERVAL_SECS` | `21600` (6 h) | How often to run a sweep. Clamped to a 60 s floor. |
| `FLEETY_BACKUPS_RETENTION_SECS` | `604800` (7 d) | Backup directories older than this are deleted. |
| `FLEETY_HISTORY_ROTATE_BYTES` | `33554432` (32 MiB) | When a device's `history.jsonl` crosses this size, it's renamed to `history.jsonl.<unix_ts>` (archive kept; live file resets). |

## mDNS service discovery

Server announces `_fleety._tcp.local.`; CLI / fleetyd browse for it as the
last fallback when no URL is configured.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_MDNS_DISABLED` | (unset) | Set anything to skip both announce and browse. Useful on corporate networks that block mDNS. |
| `FLEETY_MDNS_HOST_IP` | (auto) | Force the advertised IP. **Required when `FLEETY_ADDR` binds to `0.0.0.0`** — the server doesn't enumerate interfaces. |
| `FLEETY_MDNS_HOST` | hostname / `COMPUTERNAME` / `HOSTNAME` | mDNS instance name. |

## Daemon (`fleetyd`)

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_AGENT_URL` | mDNS → `ws://127.0.0.1:8787` | Server WebSocket URL. Tries mDNS (2 s) before falling back to localhost. |
| `FLEETY_DEVICE_ID` | hostname / `COMPUTERNAME` / `HOSTNAME` / `fleetyd-device` | This device's id (path-safe; no slashes / `:`). |
| `FLEETY_DEVICE_ROOT` | cwd | Filesystem root the on-device tools operate within. |
| `FLEETY_TOKEN` | (unset, then `~/.fleety/fleetyd.token`) | Auth token. fleetyd persists a freshly-paired one to `~/.fleety/fleetyd.token`; this env var overrides. |
| `FLEETY_PAIRING_CODE` | (unset) | Pass once to enroll a new device; server mints a token in `Welcome`, fleetyd writes it to disk. |

## Background service lifecycle (`fleetyd` and `fleety-server`)

Both binaries run as a background OS service (no window, survive the terminal
closing, single-instance) via the platform service manager — **systemd `--user`**
(Linux), **launchd LaunchAgent** (macOS), the **Service Control Manager**
(Windows). CLI verbs map to the manager:

| Verb | Meaning |
|---|---|
| `install` / `uninstall` | register / remove the service. `fleety-server install` also enables boot autostart by default; `fleetyd install` leaves autostart off until `enable`. On Windows, install/uninstall need a one-time **Administrator** terminal. |
| `start` / `stop` / `restart` | run now / stop now / restart. `restart` (and self-update) is **deferred until idle** so it never interrupts an in-flight turn (fleety-server) or a running on-device tool (fleetyd); a deadline (~300 s) and cooldown (~30 s) bound the wait. |
| `enable` / `disable` | turn boot/login autostart on / off (without uninstalling or stopping the current run). |
| `status` | report whether it is running and whether autostart is on. |
| `up` / `down` (`fleety-server` only) | `up` = install + enable + start (one command, `docker compose up -d` style); `down` = stop. |
| `run-service` | internal: the entry point the manager starts (SCM service mode on Windows). Not for manual use. |

Running with no subcommand keeps the old foreground behavior (dev): runs in the
terminal, stops on Ctrl+C.

**Windows notes:** the service runs even with **no user logged in** (`start= auto`,
boot-start). But it runs in **session 0 (no interactive desktop)**, so desktop-bound
tools (`computer_*` GUI control / screenshots, a visible browser) only work when a
user is actually logged in — headless they fail with an actionable message rather
than hanging.

## Conversation recall

The agent can search its **own user's** past conversations (per-user; conversations
are user-primary). `conversation_search` (keyword) and `conversation_list` work
with no model. `conversation_semantic_search` falls back to keyword in this build;
embedding-ranked recall (a per-user vector index reusing the wiki's embedding
model + sqlite-vec, gated by `FLEETY_WIKI_EMBED`, under `~/.fleety/agent/fleet/users/<id>/`)
is a planned follow-up and upgrades that tool in place. Every result carries a
timestamp (`ts_secs`, UTC) so time order is clear.

## Time & timezone

Timestamps are **stored as Unix epoch (UTC)** everywhere — this never changes.
Timezone is a **rendering** concern only: the agent is told the current time, and
presents timestamps, in the **acting user's** timezone. Resolution precedence:

1. the acting user's configured IANA zone (`users/<id>/timezone`),
2. else the global `FLEETY_TZ` (an IANA name like `Asia/Taipei`),
3. else UTC.

An invalid zone string falls through to the next source (never errors).

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_TZ` | (unset → UTC) | Fallback IANA timezone for rendering when a user has no configured zone. Storage stays UTC regardless. |

## Startup dependencies (auto-install on boot)

On startup, fleetyd and fleety-server each ensure the external dependencies their
features need — best-effort and **non-blocking** (a failure is logged with an
actionable message and never stops the service). All installs are root-free and
don't touch the user's system:

- **Managed runtimes** (node, python) → a portable copy under `~/.fleety/runtimes/`,
  with its bin dir prepended to the service process's own PATH (so spawned MCP
  servers / skill scripts find it). node = official portable build; python = via
  `uv`. Needs no admin.
- **User packages** (ddgs) → pipx / pip --user (unchanged).
- **Managed binaries** (insyra sidecar) → downloaded next to the executable.

Default subsets: `fleety-server` → python, ddgs, node, insyra; `fleetyd` →
insyra (add node/python via `FLEETY_DEPS` for device-side skills/MCP).

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_AUTO_INSTALL_DEPS` | (on) | Set to `0` to disable all auto-install (detect/report only) — for air-gapped / hermetic hosts. |
| `FLEETY_DEPS` | (default subset) | Comma list overriding which dependencies this binary ensures, e.g. `insyra,node,python`. |
| `FLEETY_RUNTIMES_DIR` | `~/.fleety/runtimes` | Where managed portable runtimes are installed. |
| `FLEETY_NODE_VERSION` | pinned default | Portable node version to fetch. |
| `FLEETY_DDGS_AUTO_INSTALL` | (on) | Per-dep opt-out for ddgs (`0` = don't auto-install ddgs). |
| `FLEETY_INSYRA_URL` | release asset | Override the insyra sidecar download URL. |

## Self-update polling (`fleetyd` background loop)

24h periodic check of the release manifest. Only spawns when a manifest URL is
set; without `FLEETY_AUTO_UPDATE=apply` it's notify-only (log a warning). When
`apply` installs a new binary, the service restarts itself (deferred until idle)
to run it; `fleetyd update` (one-shot) restarts the installed service the same way.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_UPDATE_MANIFEST` | (unset → no poll) | URL of the JSON manifest with `version`, `url`, `sha256`. |
| `FLEETY_UPDATE_POLL_SECS` | `86400` (24 h) | How often to check. Floor 60 s. |
| `FLEETY_AUTO_UPDATE` | `notify` | Set to `apply` to run the full update on each tick (`fleetyd update` equivalent). |

## Sidecar binaries (`fleetyd` + tools)

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_INSYRA_BIN` | (auto: beside exe) | Path to the `fleety-insyra` Go sidecar. The `insyra_exec` tool spawns this. |
| `FLEETY_INSYRA_URL` | `releases/latest/download/…` | Override the download URL for `fleetyd install` / `fleetyd update`. |

## Built-in MCP: ddgs (web search)

`ddgs` is the metasearch MCP shipped as a built-in, giving the agent
`search_text` / `search_images` / `search_news` / `search_videos` /
`search_books` / `extract_content`. **Installed and kept up to date
automatically** alongside the server:

- `scripts/install-server.sh` runs `pipx install ddgs[mcp]` on first run, and
  `pipx upgrade ddgs` (fallback `pip install -U --user`) when re-run after a
  `fleety-server` release
- The Docker image bakes `pipx install ddgs[mcp]` into the runtime layer — a
  rebuild (`docker compose up -d --build`) pulls the latest PyPI version
- At server boot, if `ddgs` is missing the runtime installs it; if already
  installed, a best-effort background `pipx upgrade` (fallback `pip -U
  --user`) refreshes it so a binary upgrade picks up the latest MCP without
  operator action
- A 24h background loop on the server upgrades the built-in MCPs to latest
  for long-running deployments that never reboot

Server seeds the entry into `{home}/mcp/builtin.json` every boot.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_DDGS_BIN` | (auto: `which ddgs`) | Absolute path to the `ddgs` binary. Useful when it's not on PATH (e.g. a venv). |
| `FLEETY_DDGS_ARGS` | `["mcp"]` | JSON array of args passed to `ddgs`. Use `["mcp","-pr","socks5h://127.0.0.1:9150"]` for ddgs's proxy mode. |
| `FLEETY_DDGS_AUTO_INSTALL` | (unset → on) | Set to `0` to **disable** the boot-time auto-install **and** the 24h auto-upgrade loop (hermetic / air-gapped hosts). Any other value (or unset) leaves both on. |
| `FLEETY_DDGS_UPGRADE_SECS` | `86400` (24 h) | Cadence of the background auto-upgrade loop. Clamped to a 60 s floor. |

## Wiki semantic search (embedding model)

`wiki_semantic_search` runs a local **EmbeddingGemma 300M** model
(`onnx-community/embeddinggemma-300m-ONNX`, Q8) in-process via fastembed/ONNX on
CPU. The model downloads once (~300MB) on first use / boot warm, then runs
offline. The vector index lives at `{wiki}/.index/embeddings.json` and stays
current automatically (re-embeds notes whose content hash changed).

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_WIKI_EMBED` | (unset → on) | Set to `0` to disable semantic search (no model download; `wiki_semantic_search` returns an error pointing at `wiki_search`). |
| `FLEETY_MODELS_DIR` | `{FLEETY_AGENT_HOME}/models` | Cache dir for downloaded model weights. |

## Tools that talk to the network

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_CHROME_URL` | `http://127.0.0.1:9222` | Chrome DevTools Protocol endpoint for `browser_*` tools. A non-loopback URL is treated as remote and never auto-provisioned. |
| `FLEETY_ALLOW_PRIVATE_NET` | `0` | Set to `1` to allow `http_request`/`fetch_url` against RFC1918 / loopback hosts. Default refuses for safety. |

## Browser / Chrome auto-provisioning

The `browser_*` (CDP) tools run on any device. When the local CDP endpoint is
down, the runtime detects an installed Chrome/Chromium and launches it headless;
if none is found it installs one (OS package manager, then a chrome-for-testing
`chrome-headless-shell` download). Runs on whichever device the tool executes.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_CHROME_BIN` | (auto: PATH / well-known paths / managed download) | Absolute path to a Chrome/Chromium (or `chrome-headless-shell`) binary. Skips detection. |
| `FLEETY_CHROME_AUTO_INSTALL` | (unset → on) | Set to `0` to disable installing/downloading Chrome when none is found (detect + launch still run). Air-gapped / hermetic hosts. |
| `FLEETY_CHROME_DIR` | `$HOME/.fleety/chrome` (or `%USERPROFILE%`) | Cache dir for managed chrome-for-testing downloads. |

## Install scripts

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_INSTALL_DIR` | `/usr/local/bin` if writable, else `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\fleety`) | Where `scripts/install.sh` / `install-server.sh` (and `install.ps1`) land the binary. Explicit value always wins. |

---

When in doubt, the source of truth for each var is its lookup site —
`grep -rn '"FLEETY_<NAME>"' crates/` will show you exactly where it's read
and what the default is.
