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
| [`crates/fleety-cli`](crates/fleety-cli) | CLI + interactive TUI (`fleety`): `init` / `ask` / `resume` / `tui`. |

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

Useful env vars: `FLEETY_MODEL_BASE_URL` / `FLEETY_MODEL` / `FLEETY_MODEL_KEY`
(+ `FLEETY_MODEL_STREAM=1`), `FLEETY_POLICY=require_approval` (gate non-read
tools), `FLEETY_ADDR`, `FLEETY_WORKSPACE`, `FLEETY_AGENT_HOME`, `FLEETY_DEVICE_ROOT`,
`FLEETY_CHROME_URL`.

## Design docs

- [`docs/spec-v0.md`](docs/spec-v0.md) — v0 scope, architecture, milestones M0–M11
- [`docs/STATUS.md`](docs/STATUS.md) — what's implemented vs. remaining (current status)
- [`docs/tools.md`](docs/tools.md) — agent tool surface
- [`prompts/`](prompts/) — protocol / memory / policy / rules (the agent system prompt)

## License

Apache-2.0 (see [`LICENSE`](LICENSE)).
