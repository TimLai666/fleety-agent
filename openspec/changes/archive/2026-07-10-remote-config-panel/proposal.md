## Why

Phase 1 gave clean, non-interactive control of the connection layer, the provider/model model, auth, and local settings. But remote *server* config is still string-in/text-out (`ConfigExec` → `ConfigResult`), so there is no way to pull the server's structured settings to the device and edit them interactively — the design's headline "one panel to set anything" (§7) is impossible on the current wire. Phase 2 (design §7/§8, red-team blockers M3/M4) adds the structured channel and the all-in-one interactive panel, delivering goal G2.

## What Changes

- **Structured config wire (`ConfigSnapshot`/`ConfigApply`).** New frames pull the server's settings as structured `ConfigEntry`s (key, scope, value, default, description, secret, is_set, effect, choices) plus the structured provider/model sub-config, and push a sparse set of `ConfigChange`s back. Applying carries a `base_revision` (file hash + server boot id) for optimistic locking — a stale base returns a conflict instead of a lost update (M3). Secret changes are tri-state (`keep`/`set`/`clear`): masked values are never echoed back, and only a real new value is sent (M3).
- **Real atomic server save.** The server's `config.toml` write moves to temp + rename under a single per-file mutex (like providers.toml); a broken file is a clear error, never a fail-soft revert to defaults (M3).
- **Forward-compatible protocol.** `Welcome` gains an additive capability/protocol-version field; the CLI uses it to pick the structured `Snapshot`/`Apply` path or fall back to the old `ConfigExec`. An unknown inbound frame stops disconnecting the connection — the server replies with an `unsupported` error frame and stays connected (M4), so future additive frames never break a live link. `PROTOCOL_VERSION` is bumped.
- **Interactive all-in-one panel.** Bare `fleety config` on a TTY opens a three-region ratatui panel (Tab: Connection / This device / Server) — no `--target` needed. Region 1 edits `connections.toml` (Phase 1), region 2 the local Cli/Shared settings (Phase 1), region 3 the connected server's settings + provider/model via the new structured channel (remote edit). Secrets are masked and write-only; effect timing is shown; providers render per `type`.
- **Sensitive-key authorization (§4/§8).** A `ConfigApply` that mutates a Server-scope key requires auth on (leaning on auth-default-on); overwriting a "data-exfiltration" key (provider `base_url`/`key`, backup repo/token, oauth endpoints) shows a prominent confirm + audits the change (old/new host); `ConfigSnapshot` returns only `is_set` for secrets and records who read.

## Non-Goals (optional)

- Owner/normal device tiering (design keeps it a future advanced option).
- Pairing-code hardening (longer codes, single active code, redeem rate-limit) and a hard `wss`/TLS transport requirement — these §4 defenses are valuable but a separate hardening change; this change requires auth for sensitive mutation but does not add TLS enforcement.
- Any change to the Phase 1 non-interactive command surface (it stays; the panel is additive).

## Capabilities

### New Capabilities

- `structured-config-protocol`: `ConfigSnapshot`/`ConfigApply` frames with revision optimistic-locking, secret tri-state, capability negotiation, unknown-frame tolerance, and atomic server config save.
- `interactive-config-panel`: the bare-`fleety config` three-region interactive panel that edits connection, local, and (remotely) server settings from one entry point.

### Modified Capabilities

(none)

## Impact

- Affected specs: `structured-config-protocol` (new), `interactive-config-panel` (new)
- Affected code:
  - New: crates/fleety-cli/src/config_panel.rs (the three-region interactive panel)
  - Modified: crates/fleety-protocol/src/lib.rs (ConfigSnapshot/ConfigSnapshotResult/ConfigApply frames + ConfigEntry/ConfigChange types + Welcome capability field + PROTOCOL_VERSION bump + unknown-frame tolerance), crates/fleety-server/src/conn.rs (snapshot builder + apply handler with revision lock/tri-state/auth gate + unknown-frame reply + Welcome capability), crates/fleety-tools/src/config.rs (structured snapshot entries + atomic mutexed save + revision hash), crates/fleety-tools/src/providers_config.rs (structured provider/model snapshot for the panel), crates/fleety-cli/src/config.rs (capability detection + Snapshot/Apply flow; bare `fleety config` opens the panel on a TTY), crates/fleety-cli/src/main.rs (route bare `config` to the panel), docs/roadmap.md, docs/STATUS.md, docs/design-cli-config.md
  - Removed: (none)
