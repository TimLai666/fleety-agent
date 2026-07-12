# Fleety

Fleety is a real J.A.R.V.I.S. — a cross-device, full-access agent and device-fleet
assistant. Summon the agent from any device; it knows where the message came from,
what each device can do, and routes each task to the device best able to finish it.

> **Status: v0 implemented.** Working cross-device agent — WebSocket server +
> agent loop, CLI and interactive TUI, on-device execution (client_session bridge
> + SSH), browser automation (CDP), scheduling, skills/MCP/wiki, and fleetyd
> connect/autostart/self-update. See [`docs/STATUS.md`](docs/STATUS.md) for the
> full picture; design is in [`docs/spec-v0.md`](docs/spec-v0.md).

## Install

Install the `fleety` CLI with one line — it fetches the latest release for your
platform and puts `fleety` on your PATH:

**macOS / Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/TimLai666/fleety-agent/main/scripts/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/TimLai666/fleety-agent/main/scripts/install.ps1 | iex
```

Then point it at your agent and chat:

```sh
fleety init          # scan the LAN, pick a server from the list, pair — one flow
fleety tui           # interactive UI  (or: fleety ask "hello")
# (or point it somewhere explicitly: fleety init ws://your-agent-host:8787)
```

Override the install location with `FLEETY_INSTALL_DIR`. The one-liners pull from
the newest [GitHub Release](https://github.com/TimLai666/fleety-agent/releases) —
maintainers cut one by pushing a tag (`git tag v0.1.0 && git push origin v0.1.0`),
which triggers [`.github/workflows/release.yml`](.github/workflows/release.yml) to
build and attach the per-platform binaries (`fleety`, `fleety-server`, `fleetyd`).

## Deploy the server

**Docker (recommended)** — build and run in one command:

```sh
docker compose up -d --build
```

Listens on `:8787`, persists state in the `fleety-data` volume, and operates on
`./workspace`. Configure a model and policy via env — see
[`docker-compose.yml`](docker-compose.yml).

**Without Docker** — one-line install of the `fleety-server` binary:

```sh
curl -fsSL https://raw.githubusercontent.com/TimLai666/fleety-agent/main/scripts/install-server.sh | sh
```

It installs `fleety-server` onto your PATH and prints how to run it; register
it as a boot service with `fleety-server up` (systemd --user / launchd / SCM).

## Workspace

| Crate | Role |
|---|---|
| [`crates/agent-core`](crates/agent-core) | Generic agent core: the never-crash tool-calling loop, `ModelProvider` (OpenAI-compatible), approval gating, context compaction, errors/observability. The future standalone framework — depends on no Fleety crate. |
| [`crates/fleety-protocol`](crates/fleety-protocol) | Wire types shared by CLI / daemon / server (incl. the on-device `RunTool`/`ToolResult` frames). |
| [`crates/fleety-tools`](crates/fleety-tools) | Shared, root-relative workspace tools (read/list/search-ripgrep/write/edit/run/git + unified diff). Used by the server **and** the daemon, so every device gets the full toolset. |
| [`crates/fleety-server`](crates/fleety-server) | Fleety Agent server (`fleety-server`): runs the agent loop, the tool surface, cross-device routing, and the scheduler. |
| [`crates/fleety-daemon`](crates/fleety-daemon) | Device background service (`fleetyd`): connects, runs on-device tools, `install`/`update` (also provisions the `fleety-insyra` sidecar so `insyra_exec` works on the device). |
| [`crates/fleety-cli`](crates/fleety-cli) | CLI + interactive TUI (`fleety`): `init` / `ask` / `resume` / `conversations` / `tui` / `voice` / `status` / `config` / `audit` / `rollback` / `daemon` / `update` / `acp` / `pair` / `pair-code` (see [Command reference](#command-reference)). |

Dependency rule: everything may depend on `agent-core`; `agent-core` depends on
nothing Fleety-specific, so it can later be extracted to its own repo and mounted
back as a git submodule.

Sidecars live under [`sidecars/`](sidecars): [`fleety-insyra`](sidecars/fleety-insyra)
is a small Go process wrapping the [Insyra](https://github.com/HazelnutParadise/insyra)
data-analysis DSL, driven by the `insyra_exec` tool over stdin/stdout JSON.

## What it can do

The agent exposes 80+ tools: workspace files + git (`read_file`, `list_dir`,
`search_files`, `write_file`, `edit_file`, `delete_file`, `move_file`, `make_dir`,
`rollback`, `run_command`, `git_*`) — mutations back up + return a unified diff
(any device, not just git repos), `run_command` can `track` paths to diff what a
command changed, and `rollback` restores from a backup — plus memory and
audit history, **data analysis** via the Insyra DSL (`insyra_exec` — stateful
`.isr` sessions backed by the `fleety-insyra` Go sidecar), a knowledge wiki,
HTTP (`fetch_url` / `http_request`), self-managed
scheduling (`schedule_*` with a fire loop + per-schedule mandate), a skills + MCP
runtime, and **cross-device execution** — run tools on another connected device
(`device_exec`, via the daemon), over SSH (`ssh_exec`), or drive a Chrome over the
DevTools Protocol (`browser_navigate` / `browser_eval` / `browser_screenshot`).
Also: **subagent delegation** (spawn focused helpers, optionally on a cheaper
economy model tier), goal tracking (`set_goal` / `complete_goal`), **semantic
recall across past conversations**, interactive PTY terminal sessions, and
opt-in presence/site awareness for the fleet.
Safety throughout: risk classes + approval gating, workspace path-escape and SSRF
guards, rollback backups, and device-scoped handles. The loop never crashes —
errors come back as messages.

## Build & test

```sh
cargo build --workspace
cargo test --workspace
```

## Run

```sh
# Server (listens on 0.0.0.0:8787 by default — reachable across devices; auth
# is required by default). With no model env set it echoes; point it at any
# OpenAI-compatible endpoint to use a real model:
FLEETY_MODEL_BASE_URL=http://localhost:1234/v1 FLEETY_MODEL=your-model \
  cargo run -p fleety-server

# Client (separate shell): save the URL, then chat.
cargo run -p fleety-cli -- init ws://127.0.0.1:8787
cargo run -p fleety-cli -- tui        # or: ask "hello"  /  resume <conversation_id>

# A device daemon (optional): connect a device so the agent can operate it.
cargo run -p fleety-daemon            # fleetyd install → autostart; fleetyd update → self-update
```

Common env vars: `FLEETY_MODEL_BASE_URL` / `FLEETY_MODEL` / `FLEETY_MODEL_KEY`
(+ `FLEETY_MODEL_STREAM=1`), `FLEETY_POLICY=require_approval` (gate non-read
tools), `FLEETY_ADDR`, `FLEETY_WORKSPACE`, `FLEETY_AGENT_HOME`.

See [`docs/env.md`](docs/env.md) for the **full reference** — every
`FLEETY_*` variable the runtime reads, grouped by binary (server, daemon,
sidecars, CLI). Includes mDNS discovery, retention / GC, self-update
polling, and on-device tool routing.

## Connecting & configuring

### Use Fleety from an editor (ACP)

An ACP-capable editor (Zed, …) can drive Fleety from its agent panel: it launches
`fleety acp` as a subprocess, which bridges to your server. Auto-configure Zed
with `fleety acp install zed`. Full guide (setup, other editors, remote server,
updates, troubleshooting): [`docs/acp.md`](docs/acp.md).

### Point the CLI at a server

`fleety` resolves the server URL in this order:

1. a one-shot `-s <name>` / `--url <ws>` override, else
2. `FLEETY_AGENT_URL` (env, transient), else
3. the current server profile in `~/.fleety/connections.toml` (set by
   `fleety server use` / `fleety init`), else
4. mDNS discovery on the LAN (a short 2 s probe; sticky once enrolled), else
5. the local default `ws://127.0.0.1:8787`.

So on one machine `fleety tui` just works. For a remote server the easiest path
is bare `fleety init` on a TTY: it scans the LAN, lists every announced server by
name (marking ones you already saved), lets you pick, saves the profile, and
prompts for the pairing code in one flow. Or point it explicitly with
`fleety init ws://host:8787` (or `fleety server add <name> <url> --use`) — every
later command uses the saved profile. Auth is **required by default**, so enroll
this device with a pairing code. Mint one on the **server host** with
`fleety pair-code` — same-host loopback trust means it needs no auth there, and
it prints the exact `fleety pair <code>` to run on the new device (from an
already-paired device it works too with that device's token; the agent's
`pair_create` tool mints one in-conversation as well). A fresh server also prints
a first-run code at startup. The very first device can instead connect with the
server's bootstrap admin token (`FLEETY_TOKEN`, the same value set on the server)
and pair the rest from there.

### Configure the model (server side)

The server talks to any OpenAI-compatible endpoint (or a native Gemini one).
Three ways, in increasing power:

1. **Env (quickest)** — `FLEETY_MODEL_BASE_URL` + `FLEETY_MODEL` (+ optional
   `FLEETY_MODEL_KEY`, `FLEETY_MODEL_STREAM=1`). With none set the server echoes
   (an offline stub), so it always boots.
2. **Persisted config** — `fleety-server config set FLEETY_MODEL <name>` (plus
   `… set FLEETY_MODEL_BASE_URL …` / `… set FLEETY_MODEL_KEY …`). Stored in the
   server host's `~/.fleety/config.toml` and seeded into the environment at boot
   (an explicit env var still wins). A cheaper economy tier for subagents lives
   in the `FLEETY_CHEAP_MODEL_*` twins.
3. **Two-tier providers/models (`providers.toml`)** — when you have **several
   accounts or endpoints** (e.g. multiple Codex accounts), or want one provider to
   serve different models to different roles. Define `[providers.<name>]` tagged by
   `type` (`api` = base_url + key; `oauth:codex` = a per-provider OAuth login), then
   `[models.main]` / `[models.cheap]` as pools whose `members` each name a provider
   + model (+ `stream`/`modalities`/`effort`) with a `single`/`round_robin`/`failover`
   strategy. With none configured, the env vars above seed `main`; a **present but
   broken** file makes the server refuse to boot (not a silent echo fallback). See
   [`docs/env.md`](docs/env.md#named-provider-pool-providerstoml).

### Sign in with ChatGPT instead of an API key (Codex OAuth)

Use a ChatGPT subscription rather than a static key:

1. Add an `oauth:codex` provider (name it, e.g. `codex1`) in `providers.toml`,
   then `fleety auth login codex1` — opens the browser (or
   `fleety auth login codex1 --no-browser` prints the URL), captures the redirect
   on the fixed loopback port `http://localhost:1455/auth/callback` (must be free
   during login), and **delivers the tokens to the connected server**, which
   stores them **per provider** at its `~/.fleety/codex-oauth/<provider>.json`
   (0600 on Unix; refreshed automatically, never printed — nothing is persisted
   on the CLI host). Requires a paired, up-to-date server. Each `oauth:codex`
   provider holds its **own** account, so different providers can sign in to
   different accounts, and re-running `login` on one switches its account. The
   public Codex client id is the default — no setup needed.
2. That provider (`type = "oauth:codex"`) then calls the ChatGPT/Codex backend
   over the **Responses API** with its own account's token (auto-refreshed) — no key.

`fleety auth status` lists each `oauth:codex` provider's sign-in state (or
`fleety auth status <provider>` for one); `fleety auth logout <provider>` clears
that provider's credential. You can also do all of this from `fleety config`
(Providers → select the codex provider → `e` → sign in / out / switch account).
Upgrading to per-provider Codex clears any old global login — sign in again per provider.

> **Note:** end-to-end behavior against the live Codex backend is network-gated
> and unverified from CI (the request/SSE shapes follow the documented Codex CLI
> contract and are unit-tested offline). See
> [`docs/env.md`](docs/env.md#codex-chatgpt-oauth-sign-in-instead-of-an-api-key).

### Where config commands apply (`--target`)

By default `fleety config …` manages the **connected server's** config over the
connection — set the server's model from your laptop without shell access to the
server host. Use `--target` to choose:

- `--target server` (default) — the connected server. Requires an authenticated
  connection; the result says when it takes effect (a provider/model change on the
  next connection; a flat `FLEETY_*` setting after a server restart). A mutating
  change is refused when the server runs with auth disabled.
- `--target local` — this CLI host's own `~/.fleety` files (no connection), scoped
  to this device's own settings (Cli/Shared); a Server-scoped key is redirected to
  the server.
- `--target <device-id>` — a specific device. *Follow-up* — the server currently
  reports this as not-yet-supported; configure the device on its own host with
  `fleetyd config` for now.

If the server can't be reached, the CLI says so and suggests `--target local`.
`fleety-server config …` (run on the server host) remains as a bootstrap path.

### Edit config interactively

On a TTY:

- **`fleety config`** (no args) — a guided **menu**: pick Providers, Models, or
  Settings and drill in. Providers/Models open the provider editor (add a
  provider by type with per-field prompts; set a model role by picking a provider
  then choosing from its `/models` list, or typing an id if the fetch is
  unavailable). Settings is the three-region panel (Connection / This device /
  Server). The Server region edits the connected server's settings — including
  providers/models — live **over the connection** (optimistic-locked; secrets
  stay write-only), so remote editing no longer needs shell access to the server
  host. Editing `FLEETY_TZ` opens a searchable IANA timezone picker (or follow
  the host device). The key hints stay visible; Esc steps back, q quits.
- `fleety config edit` — edit just the flat `FLEETY_*` settings (ratatui list;
  secrets masked; line-based fallback when not a TTY).
- `fleety config provider edit` — the provider-only editor for `providers.toml`:
  add/remove providers, set a model role's members + strategy. Saving runs the
  same validation + atomic write as the subcommands.

## Command reference

Three binaries. The **CLI** (`fleety`) is what you talk to the agent with; the
**server** (`fleety-server`) runs the agent; the **daemon** (`fleetyd`) connects a
device so the agent can operate it. Server and daemon share the same
service-lifecycle verbs (they register with the OS service manager — systemd /
launchd / Windows SCM).

### `fleety` — the CLI

| Command | What it does |
|---|---|
| `fleety init <ws-url>` | Point this device at a server (e.g. `ws://host:8787`) for later commands — sugar for `fleety server add default <url> --use`. |
| `fleety server <add\|use\|list\|show\|current\|rename\|remove\|set-url>` | Manage the server profiles this device can connect to (`~/.fleety/connections.toml`). `use` switches the current one; `add … --use` adds and switches. |
| `fleety ask "<text>"` | One-shot prompt; prints the reply. Accepts file paths as attachments. |
| `fleety resume <conversation_id>` | Continue an existing conversation. |
| `fleety conversations [<limit>]` | List your recent conversations (most-recent-first) with a relative last-activity time and a first-message preview, so you can find the id `resume` needs. |
| `fleety tui` | Interactive terminal UI (streaming chat). While a reply is generating, **Esc cancels** the turn (completed work is kept); when idle, Esc quits. Ctrl+C always quits. PgUp/PgDn scroll the history. |
| `fleety voice` | Voice conversation (speech-to-text in, spoken reply out). |
| `fleety status` | Server health: version, uptime, connected devices. |
| `fleety config <list\|get\|set\|unset\|edit>` | Inspect/edit settings; secrets masked. Targets the connected **server** by default; `--target local` edits this host's `~/.fleety/config.toml`. `edit` is local + interactive (ratatui on a TTY, line-based otherwise). |
| `fleety config provider\|model <…>` | Manage providers + model roles (`providers.toml`): `provider add\|set\|remove\|list`, `model set\|show\|unset\|list`. Same `--target` rule (default server). Bare `fleety config` on a TTY opens a guided menu (Providers / Models / Settings); `config provider edit` is the provider-only interactive editor. |
| `fleety audit list [<limit>]` / `fleety audit show <index>` | List this device's audit-log entries (tool calls/results/replies) / show one in full. |
| `fleety rollback list` / `fleety rollback apply <backup_id>` | List backups / restore a file from a backup. |
| `fleety pair-code` | Mint a short-lived pairing code on the connected server (loopback-trusted on the server host, else token-authed) and print the `fleety pair <code>` to run on the new device. |
| `fleety pair` | Enroll this device with a pairing code (auth-required servers). |
| `fleety daemon <verb>` | Manage the local daemon from the unified CLI — forwards to `fleetyd` (`install`/`start`/`stop`/`restart`/`status`/`update`/…). |
| `fleety update` | Update **every** fleety component installed on this host (CLI + any local server + daemon, incl. the `fleety-insyra` sidecar). One command. |
| `fleety acp` | Run as an [Agent Client Protocol](https://agentclientprotocol.com) agent over stdio (for ACP editors like Zed). Not run by hand — the editor launches it. |

### `fleety-server` — the agent server

Run with **no argument** for a foreground dev run (Ctrl+C to stop). The lifecycle
verbs register/run it as a background service:

| Command | What it does |
|---|---|
| `fleety-server up` | install + enable + start (one shot, `docker compose up -d` style). **Waits until the server is actually running** and errors if it never comes up (so a failed launch isn't reported as success). |
| `fleety-server down` | stop the running service. |
| `fleety-server install` / `uninstall` | register / remove the OS service (install also enables boot autostart). |
| `fleety-server start` / `stop` / `restart` | run now / stop now / restart. `start`/`restart` **wait for the process to actually come up** (reporting the new pid, or erroring if it doesn't) rather than returning as soon as the request is sent. A non-forced `restart` **defers until the server is idle** (no in-flight turn) instead of interrupting a turn, then waits for that deferred restart to complete (up to ~330 s); `restart --force` restarts immediately. A forced (or past-deadline, ~300 s) restart interrupts the in-flight turn, which is then recovered from the journal, not lost. |
| `fleety-server enable` / `disable` | turn boot autostart on / off. |
| `fleety-server status` | running? autostart on? |
| `fleety-server config <list\|get\|set\|unset\|edit>` | Inspect/edit **this host's** settings (e.g. `set FLEETY_MODEL …`, `set FLEETY_TOKEN …`); also `config provider\|model …` for providers + model roles. Same surface as `fleety config`, applied where the server boots. |
| `fleety-server run-service` | internal: the entry point the service manager starts. Not for manual use. |

> On Windows, `install`/`uninstall` need a one-time **Administrator** terminal.

### `fleetyd` — the device daemon

Run with **no argument** to connect in the foreground. Same lifecycle verbs as the
server, plus self-update:

| Command | What it does |
|---|---|
| `fleetyd install` / `uninstall` | register / remove the daemon service (install leaves autostart off until `enable`). |
| `fleetyd start` / `stop` / `restart` / `enable` / `disable` / `status` | as above; `start`/`restart` **wait for the daemon to actually come up** (report the new pid, or error). A manual restart is immediate; only the *self-update* path defers its restart until the daemon is idle (no running on-device tool). |
| `fleetyd config <list\|get\|set\|unset\|edit>` | Inspect/edit **this host's** settings (incl. `config provider\|model …`); same surface as `fleety config`. |
| `fleetyd update` | host-wide update: fleetyd + the `fleety-insyra` sidecar + the sibling `fleety`/`fleety-server` on this host. `fleety-server update` is the server-only-host equivalent. |
| `fleetyd run-service` | internal service entry point. |

Configuration for all three is environment-first (`FLEETY_*`) with a `config.toml`
fallback — see the **full reference** in [`docs/env.md`](docs/env.md).

The daemon and CLI connect over WebSocket, falling back automatically to **SSE +
HTTP POST** on the same port when the WebSocket can't connect (e.g. a proxy blocks
the upgrade). Force or disable it with `FLEETY_FORCE_SSE` / `FLEETY_DISABLE_SSE`
(see [`docs/env.md`](docs/env.md#transport-websocket--sse-fallback)).

## Design docs

- [`docs/spec-v0.md`](docs/spec-v0.md) — v0 scope, architecture, milestones M0–M11
- [`docs/STATUS.md`](docs/STATUS.md) — what's implemented vs. remaining (current status)
- [`docs/tools.md`](docs/tools.md) — agent tool surface
- [`docs/env.md`](docs/env.md) — every `FLEETY_*` environment variable
- [`docs/eval.md`](docs/eval.md) — offline golden-conversation harness
- [`docs/roadmap.md`](docs/roadmap.md) — open gaps + implementation plans
- [`prompts/`](prompts/) — protocol / memory / policy / rules (the agent system prompt)

## License

Apache-2.0 (see [`LICENSE`](LICENSE)).
