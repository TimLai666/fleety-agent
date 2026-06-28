## Why

Today every fleety setting is either an environment variable or buried in the connection JSON that `fleety init` writes. There is no way to *see* what is configurable, no `config get/set`, and no interactive settings screen — so configuring server, device, or CLI behavior means knowing the exact `FLEETY_*` name and exporting it in the right place. The user wants all settings reachable from the terminal, including an interactive setup interface.

## What Changes

- A **typed config registry** of known settings (name, scope, description, default) covering the existing `FLEETY_*` surface, persisted to a single `~/.fleety/config.toml` with per-scope sections (`[server]`, `[daemon]`, `[cli]`).
- **`fleety config` subcommands**: `list` (show every known setting, its scope, current value, source), `get <key>`, `set <key> <value>` (validated against the registry), `unset <key>`, all writing/reading `config.toml`.
- An **interactive settings screen** (`fleety config edit`, reusing the existing ratatui TUI) to browse settings by scope and edit values in a form, with the same validation.
- **Precedence on read**: an explicit environment variable still wins (so existing env-based deployments are unaffected), then `config.toml`, then the built-in default. The server and daemon load config at boot and use it wherever they currently read those env vars, via this precedence.
- Unknown keys are **rejected** (typo-safe); secret-bearing keys are written but masked in `list`/TUI display.

## Non-Goals

- Not removing or breaking env-var configuration — env stays the highest-precedence source.
- Not making every internal constant configurable — only a curated set mirroring the documented `FLEETY_*` settings.
- Not a remote/over-the-wire config API — this is local terminal configuration of the local config file (server reads its own file).
- Not changing the agent loop, protocol, or tool behavior.

## Capabilities

### New Capabilities

- `interactive-config`: a typed, validated config registry persisted to `~/.fleety/config.toml`; `fleety config list/get/set/unset` terminal commands and an interactive `fleety config edit` TUI; read precedence env then config then default, consumed by server and daemon at boot; unknown-key rejection and secret masking.

### Modified Capabilities

(none — existing capabilities keep their behavior; this adds a config layer feeding the same settings.)

## Impact

- Affected specs: new `interactive-config`.
- Affected code:
  - New: crates/fleety-cli/src/config.rs (config subcommands + the settings TUI screen), a shared config module under crates/fleety-tools (the typed registry + load/save + precedence resolver, so server/daemon/cli share one definition)
  - Modified: crates/fleety-cli/src/main.rs (wire `config` subcommands), crates/fleety-cli/src/tui.rs (settings screen entry), crates/fleety-server/src/main.rs (read settings via the precedence resolver at boot), crates/fleety-daemon/src/main.rs (same), docs/env.md (note that each setting is also a config key and document precedence)
  - Removed: none
