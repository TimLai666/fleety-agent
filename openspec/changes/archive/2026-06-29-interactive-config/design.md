## Context

Fleety's settings live entirely in `FLEETY_*` environment variables (documented in docs/env.md) plus a connection JSON that `fleety init` writes for the CLI. There is no inventory of settings, no get/set, and no interactive editor. The server and daemon read each env var directly at the point of use. This change adds a config layer that sits *underneath* env (env still wins) and a terminal UX to inspect and edit it, without changing what each setting does.

## Goals / Non-Goals

**Goals:**
- One typed registry of known settings (name, scope, default, description, secret?), shared by server/daemon/cli so there is a single source of truth.
- Persist to `~/.fleety/config.toml` with `[server]`/`[daemon]`/`[cli]` sections; load/save are pure-ish and testable.
- `fleety config list/get/set/unset` + an interactive `fleety config edit` TUI, both validating against the registry.
- Read precedence: env then config then default, consumed at boot; existing env deployments unaffected.

**Non-Goals:**
- Not removing env configuration; env keeps top precedence.
- Not exposing every constant; only the curated `FLEETY_*` surface.
- No remote config API; local file only.
- No agent-loop/protocol/tool changes.

## Decisions

### A single typed setting registry shared across binaries

Define, in a shared module (fleety-tools), a static registry: each entry has a `key` (the canonical name, equal to its `FLEETY_*` env name), a `scope` (Server / Daemon / Cli / Shared), a `default`, a one-line `description`, and a `secret` flag. The registry is the single source of truth that `list`, validation, the resolver, and the TUI all read. Adding a setting = one registry entry. Unknown keys passed to `set` are rejected against this registry (typo-safe).

**Alternative:** free-form key/value map — rejected (no validation, no discoverability, typo-prone).

### Persisted to `~/.fleety/config.toml`, sectioned by scope

Config lives in `~/.fleety/config.toml` (override `FLEETY_CONFIG`) with tables `[server]`, `[daemon]`, `[cli]`, `[shared]`. Load returns a map keyed by (scope, key); save writes back, preserving unknown sections is unnecessary since unknown keys are rejected on set. TOML chosen for human-editability. Load/save and the (scope,key) lookup are unit-testable without touching the real home dir (path is injectable).

**Alternative:** per-binary files / JSON — rejected (more files to manage; TOML reads better by hand).

### Read precedence: env then config then default

A resolver `resolve(key) -> String` returns the explicit env var if set and non-empty, else the config.toml value for that key's scope, else the registry default. This keeps existing env-based deployments byte-for-byte unchanged (env always wins) while letting config.toml supply values when env is unset. Server and daemon call the resolver at boot wherever they currently read these env vars; to stay minimal, the resolver may seed `std::env` for unset keys early at startup so existing `std::env::var` call sites keep working unchanged.

**Alternative:** config overrides env — rejected (would silently change behavior for operators who set env).

### `fleety config` commands + an interactive TUI editor

- `fleety config list` — every registry entry: key, scope, current value (with source: env / config / default), secrets masked.
- `fleety config get <key>` — resolved value + source.
- `fleety config set <key> <value>` — validate key against the registry, write to the key's scope section in config.toml.
- `fleety config unset <key>` — remove from config.toml (revert to env/default).
- `fleety config edit` — an interactive ratatui screen (reusing the existing TUI infra) listing settings by scope; select a setting, edit its value in a field, save writes via the same validated path. Secrets masked in the list, revealed only while editing that field.

**Alternative:** a web settings page — rejected (out of scope; this is terminal-first).

### Secrets are stored but masked in display

Entries flagged `secret` (e.g. tokens) are written to config.toml like any value but shown masked in `list` and the TUI list view. This change does not add encryption (config.toml inherits the user's home-dir permissions, same posture as today's token file).

## Implementation Contract

**Behavior:** `fleety config list` shows all known settings with current value + source; `set`/`unset` edit `~/.fleety/config.toml` after validating the key; `get` shows the resolved value. `fleety config edit` does the same interactively. At boot the server and daemon honor config.toml values for any setting whose env var is unset, while an explicit env var always wins. Unknown keys are rejected with an actionable message. Nothing panics; a missing/corrupt config file degrades to env/defaults with a warning.

**Interfaces / data shapes:**
- `Setting { key: &str, scope: Scope, default: &str, description: &str, secret: bool }`; `enum Scope { Server, Daemon, Cli, Shared }`; `fn registry() -> &'static [Setting]`.
- `fn config_path() -> PathBuf` (`FLEETY_CONFIG` override, else `~/.fleety/config.toml`).
- `load(path) -> ConfigMap` / `save(path, &ConfigMap)` (TOML); `ConfigMap` keyed by (Scope, key).
- `resolve(key, &ConfigMap) -> Resolved { value: String, source: Source }` where `Source` is Env / Config / Default; plus `seed_env_from_config(&ConfigMap)` that sets unset env keys so existing call sites keep working.
- CLI: `config list|get|set|unset|edit` parsed in main; the TUI screen in config.rs.

**Failure modes:** unknown key on get/set/unset → actionable error, no write. Invalid TOML on load → warn + treat as empty (env/defaults still work). Unwritable config path on set → actionable error, no partial write. Secret value never printed unmasked in non-edit views. Missing home dir → error with the resolved path. Never panic.

**Acceptance criteria:**
- Registry/resolver unit tests: env-set beats config beats default; source reported correctly; unknown key rejected; secret masked by a display helper.
- load/save round-trip test against a temp path (set → load → get equals; unset removes); corrupt TOML → empty + no panic.
- `config list/get/set/unset` command-dispatch tests (pure parse → action), not requiring a real TTY.
- TUI screen builds and renders a settings list (smoke test of the view construction; no interactive assertion required).
- Content review: docs/env.md notes each setting is also a config key and documents env-then-config-then-default precedence.
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.

**Scope boundaries:**
- In: shared setting registry + config.toml load/save + precedence resolver + env seeding; CLI config commands + interactive TUI editor; server/daemon boot consumption; docs; unit tests for registry/resolver/load-save/dispatch.
- Out: removing env config, encryption, remote config API, making non-registry constants configurable, agent/protocol/tool changes.

## Risks / Trade-offs

- [config silently changing operator behavior] → env always wins; config only fills unset keys.
- [secrets in a plaintext file] → same posture as today's token file (home-dir perms); masked in display; no new encryption claimed.
- [registry drift vs docs/env.md] → registry descriptions mirror env.md; docs task keeps them aligned; the registry is the single source for `list`.
- [seeding std::env at boot] → only sets keys that are unset, so it can't override an operator's env; done once, early.
- [TUI scope creep] → the edit screen reuses existing ratatui infra and the same validated set path; no new rendering framework.
