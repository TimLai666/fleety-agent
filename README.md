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
fleety chat          # terminal workspace (or: fleety ask "hello")
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
| [`crates/fleety-cli`](crates/fleety-cli) | CLI + terminal workspace (`fleety`): `init` / `ask` / `chat` / `conversations` / `connection` / `provider` / `model` / `config` / `status` / `doctor` / `completion` / `voice` / `audit` / `rollback` / `daemon` / `update` / `acp` / pairing (see [Command reference](#command-reference)). |

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

1. a one-shot `--profile <name>` or legacy `-s` / `--server <ws-url>` override, else
2. `FLEETY_AGENT_URL` (env, transient), else
3. the current server profile in `~/.fleety/connections.toml` (set by
   `fleety connection use` / `fleety init`), else
4. mDNS discovery on the LAN without borrowing stored profile credentials (a short 2 s probe), else
5. the local default `ws://127.0.0.1:8787`.

`FLEETY_AGENT_URL` never borrows credentials from a profile for a different
URL. Set `FLEETY_TOKEN` explicitly for a transient endpoint, or use a named
profile. A transient endpoint also cannot overwrite or clear another profile's
token or fingerprint.

mDNS keeps the advertised Server fingerprint only as an untrusted selection
hint. Automatic discovery never attaches a stored token or changes a
credentialed profile. A token-only profile with no URL, or a saved endpoint that
stops answering, requires explicit endpoint selection and re-pairing. Changing a
profile URL clears its old token and fingerprint first.

So on one machine bare `fleety` or `fleety chat` just works. For a remote server the easiest path
is bare `fleety init` on a TTY: it scans the LAN, lists every announced server by
name (marking ones you already saved), lets you pick, saves the profile, and
prompts for the pairing code in one flow. Or point it explicitly with
`fleety init ws://host:8787` (or `fleety connection add <name> <url> --use`) — every
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

1. Add an `oauth:codex` provider (name it, e.g. `codex1`) with
   `fleety provider add codex1 --type oauth:codex`, then
   `fleety provider login codex1` — opens the browser (or
   `fleety provider login codex1 --no-browser` copies the URL to the clipboard,
   printing it only as an explicit fallback), captures the redirect
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

`fleety provider status` lists each `oauth:codex` provider's sign-in state (or
`fleety provider status <provider>` for one); `fleety provider logout <provider>` clears
that provider's credential. You can also do all of this from **`fleety config`**
(the Providers menu): it edits the **connected server's** providers over the
connection (never a local file), so **adding an `oauth:codex` provider goes
straight into sign-in**, and selecting an existing one → `e` offers sign in /
out / switch account. Upgrading to per-provider Codex clears any old global
login — sign in again per provider.

Static Provider API keys are write-only on the connected Server. Provider
snapshots expose only whether a key exists. A blank editor field keeps it,
entering a new key replaces it, and `k` in the Provider editor or
`fleety provider set <name> --clear-key` removes it after an explicit sensitive
operation confirmation. This workflow requires config protocol 5.

> **Note:** end-to-end behavior against the live Codex backend is network-gated
> and unverified from CI (the request/SSE shapes follow the documented Codex CLI
> contract and are unit-tested offline). See
> [`docs/env.md`](docs/env.md#codex-chatgpt-oauth-sign-in-instead-of-an-api-key).

### Where config commands apply (`--owner`)

`fleety config …` routes each key to its **owning runtime**. Server settings go
to the connected server, daemon/shared settings go through that device's
`fleetyd`, and CLI settings are the only settings written by the CLI process.
Use `--owner` to make the owner explicit. Legacy `--target` remains an alias:

- `--owner server` — the connected server. Requires an authenticated
  connection; the result says when it takes effect (a provider/model change on the
  next connection; a flat `FLEETY_*` setting after a server restart). A mutating
  change is refused when the server runs with auth disabled.
- `--owner daemon` — the current device's daemon. `Shared` and `Daemon` keys are
  changed by `fleetyd` through the server's device route.
- `--owner cli` — CLI-only settings on this host. It never
  includes Shared, Daemon, Server, provider, or model settings.
- `--owner <device-id>` — that device's daemon through the server.

If an owner cannot be reached, the command fails and leaves files unchanged.
There is no fallback that writes the owner's file directly.
`fleety-server config …` (run on the server host) remains as a bootstrap path.

### Edit config interactively

On a TTY:

- **`fleety config`** (no args) — opens the shared terminal workspace at
  **Settings**, with Connection / CLI / Daemon / Server / Providers & Models
  pages. The Provider page opens the structured provider editor (add a
  provider by type with per-field prompts; set a model role by picking a provider
  then choosing from its API `/models` list or the connected server's authenticated
  Codex catalog, or typing an id if discovery is unavailable). OAuth provider rows
  show `auth=signed in`, `auth=not signed in`, or `auth=unavailable`. The Server page
  edits the connected server's settings live **over
  the connection** (optimistic-locked; secrets stay write-only), so remote editing
  no longer needs shell access to the server host. Editing `FLEETY_TZ` opens a
  searchable IANA timezone picker (or follow the host device). The key hints stay
  visible. Every owner stages and applies independently. Switching profiles
  requires Apply / Discard / Cancel for dirty remote state, then reconnects and
  reloads the selected Server and Daemon snapshots. Esc steps back; Ctrl+K opens
  commands.
- `fleety config open` — canonical explicit spelling for the same shared,
  owner-aware Settings workspace. `fleety config edit` remains an alias. Both
  require a TTY, stage edits per owner, and write only through the selected
  owner's Apply action. There is no line-based or direct-file fallback.
- `fleety provider edit` — the provider-only editor for the connected Server:
  add/remove providers, set model-role members and strategy. It uses the same
  Server-owned snapshot/apply service as commands and never edits a local
  provider file. `fleety config provider edit` is a compatibility alias.

## Command reference

Three binaries. The **CLI** (`fleety`) is what you talk to the agent with; the
**server** (`fleety-server`) runs the agent; the **daemon** (`fleetyd`) connects a
device so the agent can operate it. Server and daemon share the same
service-lifecycle verbs (they register with the OS service manager — systemd /
launchd / Windows SCM).

### `fleety` — the CLI

| Command | What it does |
|---|---|
| `fleety init <ws-url>` | Point this device at a server (e.g. `ws://host:8787`) for later commands — guided sugar for adding and selecting a connection profile. |
| `fleety connection <add\|use\|list\|show\|rename\|remove\|set-url>` | Manage the Server profiles this device can connect to (`~/.fleety/connections.toml`). `use` switches the current one; `add … --use` adds and switches. |
| `fleety ask "<text>"` | One-shot prompt; prints the reply. Accepts file paths as attachments. |
| `fleety chat` | Open the shared terminal workspace at Chat. Drafts, cursor, attachments, notices, conversation resume state, profile identity, and model context survive navigation through Conversations and Settings. |
| `fleety conversations list [--limit N]` / `fleety conversations resume <id>` | List recent conversations or continue one. Legacy `fleety conversations [N]` and `fleety resume <id>` remain accepted. |
| `fleety provider <add\|edit\|remove\|list\|login\|logout\|status>` | Manage connected-Server providers and OAuth state. |
| `fleety model <catalog\|list\|show\|set\|unset>` | Discover model IDs and manage connected-Server model roles. |
| `fleety voice` | Voice conversation (speech-to-text in, spoken reply out). |
| `fleety status` | Read CLI, local Daemon, and connected Server status. This command requires the Server status request to succeed. |
| `fleety doctor` | Run bounded read-only PASS / WARN / FAIL checks with remediation. Any FAIL exits 1. |
| `fleety completion <bash\|zsh\|fish\|powershell\|elvish>` | Write completion source to stdout without modifying shell files. |
| `fleety config <list\|get\|set\|unset\|edit>` | Inspect/edit settings; secrets masked. Auto-routes each key to Server, Daemon, or CLI ownership. `--owner server\|daemon\|cli\|<device-id>` is an owner assertion, not a file selector. |
| `fleety audit list [<limit>]` / `fleety audit show <index>` | List this device's audit-log entries (tool calls/results/replies) / show one in full. |
| `fleety rollback list` / `fleety rollback apply <backup_id>` | List or restore backups owned by the currently connected Server workspace. |
| `fleety pair-code` | Mint a short-lived pairing code on the connected server (loopback-trusted on the server host, else token-authed) and print the `fleety pair <code>` to run on the new device. |
| `fleety pair` | Enroll this device with a pairing code (auth-required servers). |
| `fleety daemon <verb>` | Manage the local daemon from the unified CLI — forwards to `fleetyd` (`install`/`start`/`stop`/`restart`/`status`/`update`/…). |
| `fleety update` | Update **every** fleety component installed on this host (CLI + any local server + daemon, incl. the `fleety-insyra` sidecar). One command. |
| `fleety acp` | Run as an [Agent Client Protocol](https://agentclientprotocol.com) agent over stdio (for ACP editors like Zed). Not run by hand — the editor launches it. |
| `fleety version` | Print the CLI version. |

Compatibility aliases map to the same typed command before any I/O:

| Compatibility spelling | Canonical spelling |
|---|---|
| `fleety tui` | `fleety chat` |
| `fleety server …` | `fleety connection …` |
| `fleety auth login\|logout\|status …` | `fleety provider login\|logout\|status …` |
| `fleety config provider …` | `fleety provider …` |
| `fleety config model …` | `fleety model …` |
| `fleety config --target …` | `fleety config --owner …` |

Global `--profile <name>` selects a saved profile for one invocation without
changing the current profile. Legacy `--server <ws-url>` is a transient raw URL.
`--json` emits `{schema_version, ok, context, data, errors}`; usage errors exit 2,
runtime/owner failures exit 1, and success exits 0. Multi-owner reads keep all
available data, set `ok: false`, include per-owner errors, label human output
`PARTIAL`, and exit 1. Mutations always resolve exactly one owner and never fall
back to direct file editing.

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

## Acknowledgements

Fleety stands on a lot of open-source work — thank you to everyone who made it.

**Designs and techniques we studied and adopted:**

- **[openclaw](https://github.com/openclaw/openclaw)** — the self-managed
  scheduler/cron design, the CDP browser automation (snapshot-refs, named
  profiles), the graceful defer-until-idle restart (ported from its
  `src/infra/restart.ts`), and the one-CLI-manages-everything control-plane style.
- **[hermes-agent](https://github.com/NousResearch/hermes-agent)** (Nous
  Research) — the knowledge wiki (Obsidian-format llm-wiki, three-layer
  structure), the agent-authored skills tier, and the post-task skill-learning
  reflection loop.
- **[headroom](https://github.com/headroomlabs-ai/headroom)** — the
  context-compression techniques (rolling summary, smart tool-result crushing,
  AST-aware code compression, cache alignment) that `agent-core` clean-room
  re-implements in Rust.
- **[picoclaw](https://github.com/sipeed/picoclaw)** (Sipeed) — the WebSocket
  heartbeat/liveness design and the push toward small-device support (sidecar
  arm64/riscv64 builds), with more of its embedded-device ideas on our roadmap.
- **[open-dynamic-workflow](https://github.com/travisliu/open-dynamic-workflow)**
  — the workflow-as-code orchestration model (`meta`/`phase()`/`agent()`/
  `parallel()`) behind Fleety's internal `run_workflow` tool.
- **[computer-use-mcp](https://github.com/domdomegg/computer-use-mcp)** — the
  design template for Fleety's native `computer_*` tools (tool surface and
  usage-restraint rules).
- **[eve](https://github.com/vercel/eve)** (Vercel) — evaluated as a possible
  base; we didn't depend on it, but borrowed its agent-as-directory idea and its
  durable-workflow checkpointing (mirrored by Fleety's event sequence + reconnect
  replay).
- **[Insyra](https://github.com/HazelnutParadise/insyra)** — the Go data-analysis
  DSL Fleety bundles as the [`fleety-insyra`](sidecars/fleety-insyra) sidecar and
  drives through the `insyra_exec` tool.
- **[claude-real-video](https://github.com/HUANGCHIHHUNGLeo/claude-real-video)** —
  the video-understanding technique behind the `video_extract` tool.
- **OpenAI's Codex CLI** — Fleety's "sign in with ChatGPT" flow and its Codex
  Responses provider follow the Codex CLI's documented OAuth + Responses
  contract; **[codex-openai-proxy](https://github.com/Securiteru/codex-openai-proxy)**
  and **[heddle](https://github.com/roackb2/heddle)** were a great help for
  cross-checking the request/header/SSE shapes and the real OAuth values.
- **[Claude Code](https://github.com/anthropics/claude-code)** (Anthropic) — a
  standing design reference and compatibility target: Fleety reuses installed
  Claude Code plugins/skills, parses its `settings.json` hooks, and mirrors its
  Workflow-tool idea.
- **[Agent Client Protocol](https://agentclientprotocol.com)** (Zed) — the editor
  protocol `fleety acp` implements, so ACP-capable editors can drive Fleety.

**Runtime tools and libraries Fleety ships or builds on** — among many:
[ddgs](https://github.com/deedy5/ddgs) (the built-in web-search MCP),
[uv](https://github.com/astral-sh/uv) (provisions the managed Python runtime),
[sqlite-vec](https://github.com/asg017/sqlite-vec) +
[fastembed-rs](https://github.com/Anush008/fastembed-rs) (local semantic search),
[tree-sitter](https://github.com/tree-sitter/tree-sitter) (AST-aware code
compression), [enigo](https://github.com/enigo-rs/enigo) +
[xcap](https://github.com/nashaofu/xcap) (the native computer-use tools),
[tokio](https://github.com/tokio-rs/tokio),
[ratatui](https://github.com/ratatui/ratatui),
[tokio-tungstenite](https://github.com/snapview/tokio-tungstenite),
[reqwest](https://github.com/seanmonstar/reqwest),
[serde](https://github.com/serde-rs/serde),
[mdns-sd](https://github.com/keepsimple1/mdns-sd) (LAN discovery),
[chrono-tz](https://github.com/chronotope/chrono-tz), and the many other crates
listed across the workspace `Cargo.toml`s.

Fleety's development itself is spec-driven with **Spectra** (SDD tooling) and
grew out of the author's earlier Python agent
([TimLai666/agent](https://github.com/TimLai666/agent)) — its memory-file model,
compaction design, and subagent spec carried over.

## License

Apache-2.0 (see [`LICENSE`](LICENSE)).
