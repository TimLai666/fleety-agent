# Fleety environment variables

The complete reference for every `FLEETY_*` variable the runtime reads.
Grouped by which binary cares about it. Anything unset uses the default.

## Server (`fleety-server`)

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_ADDR` | `127.0.0.1:8787` | WebSocket listen address. Bind `0.0.0.0:8787` to expose on the LAN. |
| `FLEETY_AGENT_HOME` | `$HOME/.fleety/agent` | Durable store root: conversations, history, backups, skills, MCP config, schedules, wiki. |
| `FLEETY_WORKSPACE` | cwd | Base directory the workspace tools (`read_file`/`write_file`/etc.) resolve **relative** paths against. |
| `FLEETY_FS_SCOPE` | (unset → `full`) | `full` (default): the structured file tools may read/write anywhere on the device (absolute paths allowed; still audited + rollback-backed; a sensitive-path guard refuses SSH keys/`/etc/shadow`/`/dev`/Windows system dirs/etc.). `workspace`: re-confine every path to the workspace/device root (`..`/absolute/symlink-tight sandbox). Set on `fleetyd` too for its `FLEETY_DEVICE_ROOT`. |
| `FLEETY_POLICY` | `full_access` | `require_approval` gates every non-read tool through the approval flow. |
| `FLEETY_REQUIRE_AUTH` | `0` | Set to `1` to require a valid token / pairing code on every `Hello`. |
| `FLEETY_TOKEN` | (unset) | Bootstrap admin token. Use it once to pair the first device. |
| `FLEETY_SCHED_TICK` | `60` | Seconds between scheduler fire-loop ticks. |

## Model provider

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_MODEL_BASE_URL` | (unset → echo) | OpenAI-compatible `/v1` root (OpenAI, OpenRouter, vLLM, Ollama, LM Studio, …). |
| `FLEETY_MODEL` | (unset → echo) | Model name to request. |
| `FLEETY_MODEL_KEY` | (unset) | Bearer token, when the endpoint needs one. |
| `FLEETY_MODEL_STREAM` | `0` | Set to `1` to use the SSE streaming endpoint (token-by-token TUI display). |

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

## Self-update polling (`fleetyd` background loop)

24h periodic check of the release manifest. Only spawns when a manifest URL is
set; without `FLEETY_AUTO_UPDATE=apply` it's notify-only (log a warning).

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
| `FLEETY_INSTALL_DIR` | `~/.fleety/bin` (or `/usr/local/bin` for root) | Where `scripts/install-server.sh` lands the binary. |

---

When in doubt, the source of truth for each var is its lookup site —
`grep -rn '"FLEETY_<NAME>"' crates/` will show you exactly where it's read
and what the default is.
