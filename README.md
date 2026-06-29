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
fleety init ws://your-agent-host:8787   # save the agent URL
fleety tui                              # interactive UI  (or: fleety ask "hello")
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

It installs `fleety-server` onto your PATH and prints how to run it plus a
ready-to-use systemd unit for autostart.

## Workspace

| Crate | Role |
|---|---|
| [`crates/agent-core`](crates/agent-core) | Generic agent core: the never-crash tool-calling loop, `ModelProvider` (OpenAI-compatible), approval gating, context compaction, errors/observability. The future standalone framework — depends on no Fleety crate. |
| [`crates/fleety-protocol`](crates/fleety-protocol) | Wire types shared by CLI / daemon / server (incl. the on-device `RunTool`/`ToolResult` frames). |
| [`crates/fleety-tools`](crates/fleety-tools) | Shared, root-relative workspace tools (read/list/search-ripgrep/write/edit/run/git + unified diff). Used by the server **and** the daemon, so every device gets the full toolset. |
| [`crates/fleety-server`](crates/fleety-server) | Fleety Agent server (`fleety-server`): runs the agent loop, the tool surface, cross-device routing, and the scheduler. |
| [`crates/fleety-daemon`](crates/fleety-daemon) | Device background service (`fleetyd`): connects, runs on-device tools, `install`/`update` (also provisions the `fleety-insyra` sidecar so `insyra_exec` works on the device). |
| [`crates/fleety-cli`](crates/fleety-cli) | CLI + interactive TUI (`fleety`): `init` / `ask` / `resume` / `tui` / `voice` / `status` / `config` / `audit` / `rollback` / `daemon` / `update` / `acp` / `pair` (see [Command reference](#command-reference)). |

Dependency rule: everything may depend on `agent-core`; `agent-core` depends on
nothing Fleety-specific, so it can later be extracted to its own repo and mounted
back as a git submodule.

Sidecars live under [`sidecars/`](sidecars): [`fleety-insyra`](sidecars/fleety-insyra)
is a small Go process wrapping the [Insyra](https://github.com/HazelnutParadise/insyra)
data-analysis DSL, driven by the `insyra_exec` tool over stdin/stdout JSON.

## What it can do

The agent exposes ~42 tools: workspace files + git (`read_file`, `list_dir`,
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
# Server (defaults to ws://127.0.0.1:8787). With no model env set it echoes;
# point it at any OpenAI-compatible endpoint to use a real model:
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

### Point the CLI at a server

`fleety` resolves the server URL in this order:

1. `FLEETY_AGENT_URL` (env), else
2. the URL saved by `fleety init <ws-url>` (in `~/.fleety/config.json`), else
3. mDNS discovery on the LAN (a short 2 s probe), else
4. the local default `ws://127.0.0.1:8787`.

So on one machine `fleety tui` just works; for a remote server run
`fleety init ws://host:8787` once and every later command uses it. If the server
requires auth (`FLEETY_REQUIRE_AUTH=1`), enroll this device with
`fleety pair <code>` — the code comes from `pair_create` on an already-paired
device, or the server console for the very first device.

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
3. **Named provider pool (`providers.toml`)** — when you have **several accounts
   or endpoints** of the same model (e.g. multiple Codex accounts). Define
   `[[provider]]` entries, pool them in a `[[group]]` with a `round_robin`
   (spread quota) or `failover` (primary + backups) strategy, and map roles
   (`main` / `cheap` / any subagent tier) to a provider or group. On any error
   the pool moves to the next member. Absent or broken → it falls back to the env
   vars above, so zero-config behavior is unchanged. See
   [`docs/env.md`](docs/env.md#named-provider-pool-providerstoml).

### Edit config interactively

On a TTY:

- `fleety config edit` — edit the flat `FLEETY_*` settings (ratatui list; secrets
  masked; line-based fallback when not a TTY).
- `fleety config provider edit` — list and edit `providers.toml`: add/remove
  providers, set a group's members + strategy, bind roles. Saving runs the same
  validation + atomic write as the subcommands.

Non-interactively, manage the pool with
`config provider|group|role <…>` (below). `fleety-server config …` and
`fleetyd config …` expose the **same** surface, applied on the host where each
binary runs.

## Command reference

Three binaries. The **CLI** (`fleety`) is what you talk to the agent with; the
**server** (`fleety-server`) runs the agent; the **daemon** (`fleetyd`) connects a
device so the agent can operate it. Server and daemon share the same
service-lifecycle verbs (they register with the OS service manager — systemd /
launchd / Windows SCM).

### `fleety` — the CLI

| Command | What it does |
|---|---|
| `fleety init <ws-url>` | Save the agent URL (e.g. `ws://host:8787`) for later commands. |
| `fleety ask "<text>"` | One-shot prompt; prints the reply. Accepts file paths as attachments. |
| `fleety resume <conversation_id>` | Continue an existing conversation. |
| `fleety tui` | Interactive terminal UI (streaming chat). |
| `fleety voice` | Voice conversation (speech-to-text in, spoken reply out). |
| `fleety status` | Server health: version, uptime, connected devices. |
| `fleety config <list\|get\|set\|unset\|edit>` | Inspect/edit flat settings (`~/.fleety/config.toml`); secrets masked. `edit` opens an interactive screen (ratatui list on a TTY, line-based otherwise). |
| `fleety config provider\|group\|role <…>` | Manage the named provider pool (`providers.toml`): `provider add\|set\|remove\|list`, `group set\|remove\|list`, `role set\|unset\|list`. `config provider edit` opens an interactive editor on a TTY. |
| `fleety audit [device]` | List a device's audit-log entries (tool calls/results/replies). |
| `fleety rollback <...>` | List backups / restore a file from a backup. |
| `fleety pair` | Enroll this device with a pairing code (auth-required servers). |
| `fleety daemon <verb>` | Manage the local daemon from the unified CLI — forwards to `fleetyd` (`install`/`start`/`stop`/`restart`/`status`/`update`/…). |
| `fleety update` | Update **every** fleety component installed on this host (CLI + any local server + daemon, incl. the `fleety-insyra` sidecar). One command. |
| `fleety acp` | Run as an [Agent Client Protocol](https://agentclientprotocol.com) agent over stdio (for ACP editors like Zed). Not run by hand — the editor launches it. |

### `fleety-server` — the agent server

Run with **no argument** for a foreground dev run (Ctrl+C to stop). The lifecycle
verbs register/run it as a background service:

| Command | What it does |
|---|---|
| `fleety-server up` | install + enable + start (one shot, `docker compose up -d` style). |
| `fleety-server down` | stop the running service. |
| `fleety-server install` / `uninstall` | register / remove the OS service (install also enables boot autostart). |
| `fleety-server start` / `stop` / `restart` | run now / stop now / restart (restart defers until idle — never interrupts a turn). |
| `fleety-server enable` / `disable` | turn boot autostart on / off. |
| `fleety-server status` | running? autostart on? |
| `fleety-server config <list\|get\|set\|unset\|edit>` | Inspect/edit **this host's** settings (e.g. `set FLEETY_MODEL …`, `set FLEETY_TOKEN …`); also `config provider\|group\|role …` for the provider pool. Same surface as `fleety config`, applied where the server boots. |
| `fleety-server run-service` | internal: the entry point the service manager starts. Not for manual use. |

> On Windows, `install`/`uninstall` need a one-time **Administrator** terminal.

### `fleetyd` — the device daemon

Run with **no argument** to connect in the foreground. Same lifecycle verbs as the
server, plus self-update:

| Command | What it does |
|---|---|
| `fleetyd install` / `uninstall` | register / remove the daemon service (install leaves autostart off until `enable`). |
| `fleetyd start` / `stop` / `restart` / `enable` / `disable` / `status` | as above (restart defers until any running on-device tool finishes). |
| `fleetyd config <list\|get\|set\|unset\|edit>` | Inspect/edit **this host's** settings (incl. `config provider\|group\|role …`); same surface as `fleety config`. |
| `fleetyd update` | self-update to the latest release (also refreshes the `fleety-insyra` sidecar). For a host-wide update of all components, prefer `fleety update`. |
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
