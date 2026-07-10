# Fleety environment variables

> **Config file.** The most-used `FLEETY_*` settings are also **config keys** you
> can set without exporting env: `fleety config list` / `get` / `set <KEY>
> <VALUE>` / `unset`, or `fleety config edit` interactively. Values persist to
> `~/.fleety/config.toml` (override with `FLEETY_CONFIG`), sectioned by scope
> (`[server]` / `[daemon]` / `[cli]` / `[shared]`). **Read precedence is env →
> config → default**: an explicit environment variable always wins, so config
> only fills what env leaves unset; the server and daemon load it at boot.
> Secret-flagged keys (tokens/model keys) are masked in `list`/`edit`.
> `fleety config list` shows exactly which keys are settable this way —
> variables not in that list are **env-only** (`config set` rejects them with
> `unknown setting`).

The complete reference for every `FLEETY_*` variable the runtime reads.
Grouped by which binary cares about it. Anything unset uses the default.

## Server (`fleety-server`)

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_ADDR` | `0.0.0.0:8787` | WebSocket listen address. Defaults to all interfaces so it's reachable across devices out of the box (auth is required by default). Set `127.0.0.1:8787` for loopback-only. |
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
| `FLEETY_POLICY` | `full_access` | `require_approval` gates every non-read tool through the approval flow. Limitation: under `require_approval` the server does not read frames mid-turn (the approval gate owns the inbound stream), so a `CancelTurn` sent during a gated turn has no effect — cancel works under the default full-access policy. |
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
| `FLEETY_VOICE_AUDIO` | `auto` | Voice transport. `auto`: send the captured audio to the model when it accepts audio input (the server advertises this on connect), else transcribe locally. `on`: always send audio. `off`: always transcribe locally (the prior behavior). Sent audio is a compact 16 kHz mono WAV; an unknown value is treated as `auto`. |
| `FLEETY_VOICE_AUDIO_MAX_KB` | `2048` | Max audio payload (KB); a larger capture falls back to local transcription. |

## Model provider

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_MODEL_BASE_URL` | (unset → echo) | OpenAI-compatible `/v1` root (OpenAI, OpenRouter, vLLM, Ollama, LM Studio, …). |
| `FLEETY_MODEL` | (unset → echo) | Model name to request. |
| `FLEETY_MODEL_KEY` | (unset) | Bearer token, when the endpoint needs one. |
| `FLEETY_MODEL_STREAM` | `0` | Set to `1` to use the SSE streaming endpoint (token-by-token TUI display). |
| `FLEETY_MODEL_RETRIES` | `3` | Retry attempts when a model call fails transiently (429, 5xx, connection/timeout). `0` disables retry (single request). |
| `FLEETY_MODEL_RETRY_BASE_MS` | `500` | Base for exponential backoff (with jitter) between model-call retries. A `Retry-After` header, when present, overrides it. |
| `FLEETY_MODEL_RETRY_CAP_MS` | `30000` | Cap on the backoff delay. |
| `FLEETY_MODEL_MODALITIES` | (name heuristic) | Comma-separated input modalities the main model accepts: `text` (implicit), `image`, `audio`, `pdf`. When unset, derived from the model name (known multimodal family → all; else text-only). Attachments of an unsupported modality degrade to a text note instead of being sent and rejected. |
| `FLEETY_CHEAP_MODEL_MODALITIES` | (name heuristic) | Same, for the economy tier. |
| `FLEETY_MODEL_EFFORT` | (none) | Default reasoning effort for the main model: `low` / `medium` / `high`. Sent only to models whose family accepts it (OpenAI o-series/gpt-5 → `reasoning_effort`; Gemini 2.5 → thinking budget); otherwise omitted. The agent can raise/lower its own effort mid-conversation, and a parent sets a subagent's effort at spawn. |
| `FLEETY_CHEAP_MODEL_EFFORT` | (none) | Default reasoning effort for the economy tier. |

Retries apply to both the main and economy tiers. Non-retryable errors (4xx other
than 429/408/425, e.g. auth/bad-request) fail fast; a streaming call only retries
before the first token is emitted.

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

### Named provider pool (`providers.toml`)

To run **more than two** providers — several Codex accounts, several
OpenAI-compatible endpoints — or to spread load / fail over across multiple
accounts of the same model, define them in `providers.toml` instead of (or
alongside) the env vars above.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_PROVIDERS` | `~/.fleety/providers.toml` | Path to the named-provider file. |

**Fallback:** when the file is absent, empty, or unparseable, the server falls
back to the env vars above (building providers named `main` and `cheap`), so the
zero-config behavior is unchanged. A broken file is ignored with a warning, not
a crash.

Each `[[provider]]` has the same fields as the env path. A `[[group]]` pools
several providers under one name with a dispatch `strategy`:

- `round_robin` — advance the starting member each call, spreading quota across
  accounts.
- `failover` — always start at the first member, advancing only on failure
  (primary + backups).

On **any** member error (after that member's own retries — see
`FLEETY_MODEL_RETRIES`), the pool tries the next member, returning an error only
once all members fail. The `[roles]` table maps a role or subagent tier name
(`main`, `cheap`, or any custom name) to a provider **or** group name; an unknown
selector falls back to `main`.

```toml
[[provider]]
name = "codex-1"
base_url = "https://api.openai.com/v1"
model = "gpt-5"
key = "sk-aaa"
stream = true            # optional (default false)
modalities = "text,image" # optional (default: model-name heuristic)
effort = "medium"         # optional (default: none)

[[provider]]
name = "codex-2"
base_url = "https://api.openai.com/v1"
model = "gpt-5"
key = "sk-bbb"

[[group]]
name = "codex"
members = ["codex-1", "codex-2"]
strategy = "round_robin"   # or "failover"

[roles]
main = "codex"            # the main turn uses the pooled group
cheap = "codex-1"         # subagents' cheap tier uses one account
```

#### Managing it with `config` (no hand-editing required)

`providers.toml` can be edited with `config` subcommands instead of by hand —
available on all three binaries (`fleety`, `fleety-server`, `fleetyd`). Each
change is validated (unique names, group members and role targets must exist)
and written atomically; an invalid change is rejected with a message and nothing
is written. Provider keys are masked in `list`.

```
config provider add codex-1 --base-url https://api.openai.com/v1 --model gpt-5 --key sk-aaa [--stream] [--modalities text,image] [--effort medium]
config provider set codex-1 --model gpt-5.1        # change only the given fields
config provider remove codex-1                     # blocked if a group/role still references it
config provider list                               # keys masked
config group set codex --members codex-1,codex-2 --strategy round_robin   # or failover
config group remove codex
config group list
config role set main codex                         # bind main → the codex group
config role unset main
config role list
```

On a TTY, `config provider edit` (CLI only) opens an interactive screen to list
and edit providers, groups, and roles; it saves through the same validation and
atomic write. Without a TTY it falls back to the subcommands above.

#### Remote vs local (`--target`)

`fleety config …` manages the **connected server's** config by default (over the
authenticated connection — no shell access to the server host needed). Pick the
host with `--target`:

- `--target server` (default) — the connected server. The result reports when the
  change takes effect: a provider-pool change (`provider`/`group`/`role`) on the
  next connection; a flat `set`/`unset` after a server restart (flat settings are
  env-seeded at boot, and the environment takes precedence).
- `--target local` — this CLI host's own `~/.fleety` files (no connection; the
  pre-existing behavior).
- `--target <device-id>` — a follow-up; the server reports it as not-yet-supported.
  Configure a device on its own host with `fleetyd config` for now.

`fleety-server config` (run on the server host) stays available as a bootstrap
path before the CLI can connect. Remote config travels only over the
authenticated connection — use TLS for remote/untrusted networks.

## Codex ChatGPT OAuth (sign in instead of an API key)

`fleety auth login` signs in to ChatGPT (OAuth 2.0 with PKCE, the same public
client id and simplified flow as the upstream Codex CLI) and stores tokens at
`~/.fleety/codex-oauth.json` (0600 on Unix), refreshed automatically and never
printed. `fleety auth status` / `fleety auth logout` manage the session. Login
uses a **fixed loopback redirect** (`http://localhost:1455/auth/callback`) because
the client id is registered with it — port 1455 must be free during login.

The defaults work out of the box; override only for a non-default install.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_CODEX_CLIENT_ID` | `app_EMoamEEZ73f0CkXaXp7hrann` | Codex OAuth public client id. |
| `FLEETY_CODEX_AUTHORIZE_URL` | `https://auth.openai.com/oauth/authorize` | Authorization endpoint. |
| `FLEETY_CODEX_TOKEN_URL` | `https://auth.openai.com/oauth/token` | Token endpoint. |
| `FLEETY_CODEX_BACKEND_URL` | `https://chatgpt.com/backend-api/codex` | Codex backend base; the provider calls `<base>/responses`. |
| `FLEETY_CODEX_ORIGINATOR` | `codex_cli_rs` | Originator sent on the Responses call (`fleety` is used on the authorize request). |
| `FLEETY_CODEX_TOKENS` | `~/.fleety/codex-oauth.json` | Override the token-store path (tests / non-default installs). |
| `FLEETY_CODEX_AUDIT` | `~/.fleety/` (auth audit file) | Override the auth-audit log path (login/logout events, never token values). |
| `FLEETY_MODEL_AUTH` / `FLEETY_CHEAP_MODEL_AUTH` | unset | Env twin of the `providers.toml` `auth` field: set `oauth:codex` to route the main / economy tier through the Codex Responses backend without a providers.toml. |

Setting a provider to `auth = "oauth:codex"` builds a **Codex Responses provider**:
it calls `<FLEETY_CODEX_BACKEND_URL>/responses` (the OpenAI Responses API, not
`/chat/completions`) with the account's OAuth bearer, the `chatgpt-account-id`
header (decoded from the login `id_token`), and the Codex beta/originator/session
headers, streaming the reply and driving tool calls. The configured
`FLEETY_MODEL_BASE_URL`/`_KEY` are ignored for this mode — Codex has its own
backend.

> **Live verification pending.** The Responses request/header/SSE shapes follow
> the documented Codex CLI contract (mirrored by codex-openai-proxy and heddle)
> and are unit-tested offline; end-to-end behavior against the real Codex backend
> is network-gated (like SSH/CDP) and unverified from CI.

## Retention / GC (server background loop)

Six-hour periodic sweep that keeps audit + backup surfaces bounded.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_GC_DISABLED` | (unset) | Set anything to skip the loop entirely. |
| `FLEETY_GC_INTERVAL_SECS` | `21600` (6 h) | How often to run a sweep. Clamped to a 60 s floor. |
| `FLEETY_BACKUPS_RETENTION_SECS` | `604800` (7 d) | Backup directories older than this are deleted. |
| `FLEETY_HISTORY_ROTATE_BYTES` | `33554432` (32 MiB) | When a device's `history.jsonl` crosses this size, it's renamed to `history.jsonl.<unix_ts>` (archive kept; live file resets). |

`FLEETY_HISTORY_ROTATE_BYTES` also rotates the presence timeline (`fleet/presence/timeline.jsonl`) past the same size.

## Presence (`fleetyd`, opt-in — off by default)

Presence tracking is off unless a device opts in. When on, `fleetyd` periodically reports a **hashed** network fingerprint (default-gateway MAC/IP + subnet — the raw values are never sent) so the server can infer which site the device is at. Server-side, each device also has its own opt-in flag (`device_set_presence_opt_in`, default off); both must be on for anything to be recorded.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_PRESENCE` | `off` | Set to `on` to enable co-location reporting from this daemon. |
| `FLEETY_PRESENCE_INTERVAL_SECS` | `300` | Seconds between co-location reports. Clamped to a 60 s floor. |

## Auto-backup to a private repo (server background loop)

Backs up the server's **non-regenerable** state to a user-configured **private**
GitHub repo. Inert unless `FLEETY_BACKUP_REPO` is set (no loop, no mirror). When
set, it commits + pushes on a schedule (and via `fleety-server backup now`),
keeping a local git mirror at `<agent_home>/../backup-mirror`.

**Scope.** Copies the agent home into the mirror **minus** re-obtainable/oversized
paths — downloaded `models`, the `skills/builtin` and `skills/synced` tiers, and
the `fleet/backups` rollback store — **plus** `config.toml` and `providers.toml`.
Everything else (conversations, memory, wiki, devices, sites, schedules,
`skills/installed`, `skills/authored`, `auth.json`, MCP config, cookies) is
included. The exclude-set (not an include-list) means new state dirs are backed
up automatically. `git` produces no commit when nothing changed, so unchanged
state is never re-pushed.

**Private-only.** Before every push, the server confirms via the GitHub API that
the target repo is `private`. If it is not private, or visibility can't be
determined, it **refuses to push**, logs a warning, and keeps the local commit.

**Restore.** `fleety-server backup restore` (run with the server stopped) clones
the repo and puts it back. It first renames the existing agent home and config
files to `.pre-restore-<timestamp>` (kept, not deleted) so the operation is
reversible, then prints a restart prompt. It never runs automatically at boot.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_BACKUP_REPO` | (unset → disabled) | Target repo as `owner/repo` or `https://github.com/owner/repo`. Unset = auto-backup entirely off. |
| `FLEETY_BACKUP_TOKEN` | (unset) | GitHub PAT used to push and to check repo visibility. Secret (masked in `config list`). |
| `FLEETY_BACKUP_INTERVAL_SECS` | `3600` (1 h) | Seconds between scheduled backups. |

> ⚠️ **Security — secrets are backed up in cleartext.** `providers.toml` (model
> API keys), `auth.json` (tokens/pairing), and cookies go into the repo **as
> plaintext**, by design. Anyone who obtains `FLEETY_BACKUP_TOKEN`, or gains
> access to the backup repo, gets **all of your secrets**. Use a **dedicated
> private repo** and a **minimally-scoped PAT** (single-repo, `contents:write`).
> The private-repo check is a guardrail against accidentally pushing to a public
> repo — it is not a substitute for protecting the token and the repo. Optional
> at-rest encryption (`FLEETY_BACKUP_PASSPHRASE`) is a future addition; the MVP
> stores cleartext.

## mDNS service discovery

Server announces `_fleety._tcp.local.`; CLI / fleetyd browse for it as the
last fallback when no URL is configured.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_MDNS_DISABLED` | (unset) | Set anything to skip both announce and browse. Useful on corporate networks that block mDNS. |
| `FLEETY_MDNS_HOST_IP` | (auto) | Override the advertised IP. When `FLEETY_ADDR` binds `0.0.0.0`, the server **auto-detects** a routable outbound IP; set this to pin a specific one (e.g. multi-homed / VPN hosts). |
| `FLEETY_MDNS_HOST` | hostname / `COMPUTERNAME` / `HOSTNAME` | mDNS instance name. |

## Daemon (`fleetyd`)

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_AGENT_URL` | mDNS → `ws://127.0.0.1:8787` | Server WebSocket URL. Tries mDNS (2 s) before falling back to localhost. |
| `FLEETY_DEVICE_ID` | OS machine id → hostname | Override for this device's id (path-safe; no slashes / `:`). See **Device identity** below. |
| `FLEETY_DEVICE_ROOT` | cwd | Filesystem root the on-device tools operate within. |
| `FLEETY_TOKEN` | (unset, then `~/.fleety/fleetyd.token`) | Auth token. fleetyd persists a freshly-paired one to `~/.fleety/fleetyd.token`; this env var overrides. |
| `FLEETY_PAIRING_CODE` | (unset) | Pass once to enroll a new device; server mints a token in `Welcome`, fleetyd writes it to disk. |

## Transport (WebSocket + SSE fallback)

The daemon and CLI connect over WebSocket by default. When the WebSocket can't
connect — most often a proxy or firewall blocking the upgrade — the client falls
back to **SSE (downstream) + HTTP POST (upstream)** against the same server port,
derived from `FLEETY_AGENT_URL` (`ws://` → `http://`, `wss://` → `https://`):
`GET /sse?session=<id>` streams `ServerMsg` frames, `POST /send?session=<id>`
carries `ClientMsg`. The server serves all three (WebSocket, SSE, POST) on one
port via axum. Auth is unchanged: the token rides the `Authorization: Bearer`
header on the SSE/POST requests (and a session can only be POSTed to by the caller
that opened it); the `Hello` frame still establishes identity. Initial pairing
uses WebSocket.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_FORCE_SSE` | `0` | Always use SSE+POST, skipping the WebSocket attempt (1/0). |
| `FLEETY_DISABLE_SSE` | `0` | Disable the fallback entirely; WebSocket only (1/0). |
| `FLEETY_SSE_TIMEOUT_SECS` | `45` | SSE half-open timeout: if no event or keep-alive arrives within this window, the client treats the stream as dead and reconnects. |

## Device identity

A device's id is a **stable, machine-derived id** — the OS machine id (Windows
`MachineGuid`, Linux `/etc/machine-id`, macOS `IOPlatformUUID`), so every process
on one machine (daemon, CLI, `fleety acp`) resolves the **same** id and two
different machines never collide. `FLEETY_DEVICE_ID` overrides it (set it on cloned
VMs/containers that share a machine id, or to pin a id); if the machine id can't be
read it falls back to the hostname with a warning (set `FLEETY_DEVICE_ID` to avoid
hostname collisions). The hostname is sent as a display **label** (shown on the
device record), not as the identity.

When authentication is on, a connection's device id is taken from its **token**
(bound at pairing), not the value asserted on the wire — so a client can't
impersonate another device; with auth off, the machine id is used directly. On
connect the server runs a **one-time, no-clobber migration**: if a device's data
still lives under its old hostname-keyed directory and the machine-id directory is
free, it moves `fleet/devices/<hostname>/` → `fleet/devices/<machine-id>/`
(atomically) and rebinds the token. Data from two machines that *already* shared a
hostname directory before the upgrade can't be un-merged; they diverge cleanly
afterward.

## Background service lifecycle (`fleetyd` and `fleety-server`)

Both binaries run as a background OS service (no window, survive the terminal
closing, single-instance) via the platform service manager — **systemd `--user`**
(Linux), **launchd LaunchAgent** (macOS), the **Service Control Manager**
(Windows). CLI verbs map to the manager:

| Verb | Meaning |
|---|---|
| `install` / `uninstall` | register / remove the service. `fleety-server install` also enables boot autostart by default; `fleetyd install` leaves autostart off until `enable`. On Windows, install/uninstall need a one-time **Administrator** terminal. |
| `start` / `stop` / `restart` | run now / stop now / restart. For **fleety-server** a non-forced `restart` **defers until idle** (no in-flight turn); `restart --force`, or passing the deferral deadline (~300 s), restarts immediately and interrupts the in-flight turn, which is then recovered from the journal, not lost. For **fleetyd** a manual `restart` is immediate; only its **self-update** path defers until idle (no running on-device tool; deadline ~300 s, cooldown ~30 s). |
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

## ACP (editor integration)

`fleety acp` makes Fleety an **Agent Client Protocol** agent: an ACP-capable
editor (e.g. Zed) launches it as a subprocess and speaks JSON-RPC 2.0 over stdio
(LSP-style `Content-Length` framing). It **bridges to the fleety-server** — it
does not run its own model — connecting at `FLEETY_AGENT_URL` (else mDNS, else
`ws://127.0.0.1:8787`), authenticating with `FLEETY_TOKEN` if set. The editor's
working directory (`session/new` cwd) becomes the conversation's workspace root
(via session-workspace-cwd). **stdout carries only JSON-RPC; logs go to stderr.**

**Editor delegation.** If the editor advertises `fs`/`terminal` client
capabilities (read from the `initialize` request), the agent additionally gets
`editor_*` tools that run **in the user's editor**: `editor_read_file` /
`editor_write_file` / `editor_edit` go through the editor's text fs (changes appear
in their buffer — may be unsaved, pending their approval), and `editor_run` runs in
the editor's terminal on its host. These are gated by what the editor advertises
(no terminal → no `editor_run`); their descriptions tell the agent to prefer them
for the user's files and that buffer edits aren't on disk until saved; results
carry a `surface`/`saved` marker. Mechanically the CLI advertises these tools in
its `Hello`; the server routes the agent's `editor_*` calls back to **that
connection** (so multiple editors on one machine don't collide), and the CLI
translates them to the editor's ACP `fs/*` / `terminal/*` methods. Disk tools
(git, search, listing) still run server-side and act on the **server's own**
files. A **conformant editor needs no changes** — only standard ACP. The live
editor round-trip is verified manually.

When the server is **remote** from the editor, the agent works on the user's
project entirely through the `editor_*` tools — `editor_run` (the editor's
terminal) covers git/build/list/etc. on the editor's host, and the fs tools cover
read/write/edit. So there is **no separate "route the session's disk tools to the
origin device" mechanism**: the editor's terminal already provides host execution.
The server-side disk tools are for the server's own filesystem, not a routing
path to the editor's host.

Tool approvals (under the require-approval policy) are surfaced to the editor via
ACP `session/request_permission`, and the user's allow/deny is relayed back to the
server; under the default full-access policy no prompts are raised.

Example editor config: run `fleety acp` as the agent command (a fleety-server
must be reachable).

## Instruction files & hooks (per-conversation project context)

The server folds `CLAUDE.md` / `AGENTS.md` from the project layers (project root
down to the origin cwd) plus the device's user-global files into each turn, and
discovers Claude-compatible hooks the same way.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_INSTRUCTION_FILE_MAX_BYTES` | `8000` | Per-file byte cap for an injected instruction file (larger files are truncated). |
| `FLEETY_INSTRUCTION_TOTAL_MAX_BYTES` | `24000` | Total byte cap across all instruction files injected in one collection. |
| `FLEETY_DISABLE_PROJECT_HOOKS` | unset | Set `1` to drop **project-scope** hooks (user-scope hooks still run) — the supply-chain kill-switch when the workspace is an untrusted repo. |

## Context compaction

When a conversation's in-context size exceeds the budget, older middle messages
are summarized into a rolling summary (system prompt + summary + recent messages
kept). This is now **incremental and cached**: the summary plus a watermark (how
many leading messages it covers) is persisted per conversation
(`<id>.compaction.json`, beside the conversation), so a follow-up turn or a reload
reuses it and only summarizes the messages added since the watermark — not the
whole middle from scratch every time. The cache is a derived optimization: it is
ignored (and a full summary recomputed) when the conversation was edited/shrank
or the compaction config changed, so it can only speed things up, never change
correctness. The full history always stays in the event log.

## Retrieving truncated tool results

Large tool results are truncated for the model (structural crush + a character
budget); the full result is always kept in the event log. The truncation marker
now names the result's id, and `fetch_tool_result(id, offset?, limit?)` returns
that full result in **bounded character windows** — it reports `total_chars` and
`next_offset` (null at the end) so the agent pages through a big result, and a
single fetch never exceeds the tool-result budget (`limit` defaults to and is
capped at it), so retrieval can't re-blow the context. Retrieval is **scoped to
the acting user**: an id from another user's conversation is reported as not
found, with no hint it exists. The audit listing (`history_list`) is filtered the
same way — only the acting user's accessible entries — closing the shared-device
leak. Tool-result audit entries are tagged with their conversation so this
scoping is enforceable; untagged (legacy/system) entries are never returned to a
specific user.

## Subagent records

A subagent run is recorded as a **child conversation** (`sub-<task_id>`) owned by
the **parent turn's acting user** (not the device owner), so its record is
user-scoped. Its events are tagged to that child conversation — so its assistant
turns are retrievable (recall / listing) and its tool output is reachable by
`fetch_tool_result` within the owning user's scope (previously they went to the
untagged device audit log and were lost to recall/fetch). The parent conversation
links to each child: the `spawn_subagent` result carries `child_conversation_id`,
the completion seed names it, and the server keeps a parent→children index, so a
conversation can enumerate the subagents it spawned and open each one's full
record. A subagent spawned by a guest (no identified user) is left unowned. This
is server-side only; the core subagent mechanism, the one-level nesting cap, and
the manager lifecycle are unchanged.

## Conversation recall

The agent can search its **own user's** past conversations (per-user; conversations
are user-primary). `conversation_search` (keyword) and `conversation_list` work
with no model. `conversation_semantic_search` is **embedding-ranked**: it embeds
the query and returns the user's most similar messages by cosine similarity
(newest-first on ties, with a `score`), backed by a **per-user vector index**
(sqlite-vec) that reuses the wiki's local embedding model (EmbeddingGemma via
fastembed, one model shared by wiki + recall, gated by `FLEETY_WIKI_EMBED`). The
index lives beside the user's conversations (`fleet/users/<id>/conversations/.index/conversations.db`),
one embedding per message, and is updated **off the turn** (fire-and-forget after
each turn, so it never adds latency) with a lazy build on first search. When
embeddings are disabled (`FLEETY_WIKI_EMBED=0`) or the index is empty/unavailable
it **falls back to keyword search** — never worse than before — and a guest (no
identified user) gets nothing. Every result carries a timestamp (`ts_secs`, UTC)
so time order is clear.

## Time & timezone

Timestamps are **stored as Unix epoch (UTC)** everywhere — this never changes.
Timezone is a **rendering** concern only: the agent is told the current time, and
presents timestamps, in the **acting user's** timezone. Resolution precedence:

1. the acting user's configured IANA zone (`users/<id>/timezone`),
2. else the global `FLEETY_TZ` (an IANA name like `Asia/Taipei`),
3. else UTC.

An invalid zone string falls through to the next source (never errors). The
per-user zone is set by the **`set_timezone`** tool (the agent calls it when the
user states their timezone/location; validated as IANA, scoped to the acting user,
unavailable to a guest).

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

## Self-update

`fleety update` is the unified, host-wide updater: it updates every fleety
component installed on the machine (the CLI itself, plus any local `fleety-server`
and `fleetyd` — the latter also refreshing the `fleety-insyra` sidecar), restarting
the services it touches. Run it on each host. `fleetyd update` (binary + sidecar)
and the daemon's background poll below are the same mechanism, scoped to the daemon.

Each binary's artifact is described by a JSON manifest (`version`, `url`, `sha256`).
`FLEETY_UPDATE_MANIFEST` gives the manifest URL; when it contains the literal
`{bin}`, that placeholder is substituted with the binary name so one base URL serves
all of them (e.g. `https://host/dl/{bin}/latest.json`). A plain URL (no `{bin}`) is
treated as the *current* binary's manifest — `fleety update` then only self-updates
the CLI (it can't safely resolve the others) and says so. Binaries ship in lockstep
(one workspace version), so the running process's version is the baseline.

**Fleet convergence (server-driven).** `Welcome` carries the server's version, so
when a device's daemon (re)connects and finds the server **newer**, it pulls this
host's binaries (fleetyd + any sibling `fleety`/`fleety-server`) to the server's
**exact** version and restarts — so a device that was offline during a `fleety
update` catches up on reconnect, and the whole fleet tracks the server rather than a
floating "latest". It is **forward-only**: a device never auto-downgrades; if a
device is newer than the server, it only warns (upgrade the server to converge).
Pinning to an exact version needs a `{version}` placeholder in the manifest URL
(e.g. `https://host/dl/{bin}/{version}/manifest.json`); without it, the daemon logs
that it can't pin. With no `FLEETY_UPDATE_MANIFEST` set, convergence is skipped.

### Background polling (`fleetyd` loop)

24h periodic check of the release manifest. Only spawns when a manifest URL is
set; without `FLEETY_AUTO_UPDATE=apply` it's notify-only (log a warning). When
`apply` installs a new binary, the service restarts itself (deferred until idle)
to run it; `fleetyd update` (one-shot) restarts the installed service the same way.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_UPDATE_MANIFEST` | (unset → no poll) | URL of the JSON manifest (`version`, `url`, `sha256`). May contain `{bin}` (serve all binaries from one base) and `{version}` (required for server-version convergence). |
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

## Command execution

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_CMD_TIMEOUT_SECS` | `120` | Wall-clock limit for `run_command` and `ssh_exec` (shared by the server and every device). On expiry the child is terminated and the result has `"timed_out": true`. `0` disables the limit; a per-call `timeout_secs` argument overrides it. These tools are **non-interactive** — they capture output and return when the process exits, so they cannot answer a prompt or drive a TUI; use non-interactive flags. |

## Interactive terminal sessions

For interactive programs (TUI / REPL / installer prompts / anything needing a
TTY) the agent uses **PTY-backed terminal sessions**: `terminal_open` starts a
process under a PTY, `terminal_input` sends input and reads the response,
`terminal_read` fetches output that arrived later, `terminal_close` ends it. The
live PTY + child persist across calls in the process's session registry (server
process for local/ssh, daemon process for a device via `device_exec`). Set
`ssh_host` on `terminal_open` to run the session on a remote host via `ssh -tt`.

It is **half-interactive**, not a real-time stream: each turn reads until the
output goes quiet, the max window elapses, or the child exits. `output` has ANSI
escapes stripped for readability; `raw_output` keeps the original bytes.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_TERMINAL_QUIET_MS` | `400` | A turn returns once output has been quiet this long (measured from the last output, or the turn start if none yet). |
| `FLEETY_TERMINAL_READ_MAX_MS` | `8000` | Hard cap on how long a single read turn waits before returning. |
| `FLEETY_TERMINAL_MAX_SESSIONS` | `8` | Max concurrent terminal sessions per process; opening beyond this errors (close one first). |
| `FLEETY_TERMINAL_IDLE_TTL_SECS` | `600` | Idle sessions are reclaimed (terminated) after this long; reaped lazily when a new session opens. |

## Synced skills (external repo)

The server keeps a fourth skill tier, `~/.fleety/agent/skills/synced`, in step with
an external skills repo at runtime — so those skills update **without** a Fleety
release. A background task syncs once at boot and then on an interval: it first
checks the repo's latest commit SHA and only downloads (the branch zip) when it
changed, then rebuilds the synced tier from the repo's top-level skill
directories (each a dir with a `SKILL.md`; loose files are ignored) and swaps it
in atomically — so added/removed skills are mirrored. The synced tier has the
**lowest precedence** (installed > authored > builtin > synced), so a same-named
installed/authored/builtin skill always wins. Any failure keeps the last good
copy and logs a warning; it never crashes. Server-side only.

| Var | Default | Meaning |
|---|---|---|
| `FLEETY_SKILLS_SYNC` | (on) | Set to `0` to disable runtime skill syncing entirely (no background task). |
| `FLEETY_SKILLS_SYNC_REPO` | `TimLai666/skills` | `owner/repo` to sync the `main` branch from. |
| `FLEETY_SKILLS_SYNC_INTERVAL_SECS` | `3600` | How often to check the repo for a new commit. |

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
