## Why

Codex OAuth is currently a single **global** credential. The client stores one token file (`~/.fleety/codex-oauth.json`), the structured-config protocol keys credentials only by `kind` ("codex-oauth") with no provider dimension, and the server keeps one token file that every `oauth:codex` provider shares. So a user cannot bind different providers to different Codex accounts, cannot switch a provider's account, and — in the new guided provider editor — adding an `oauth:codex` provider produces an entry that cannot actually be signed in from the UI. The credential must become **per provider**.

## What Changes

- Codex credentials become keyed by **provider name** end to end. Tokens still live only on the server (the client never persists them); the protocol frames, the server credential store, and the provider runtime all key by provider name, and `login <provider>` delivers that provider's tokens to the server.
- `fleety auth login <provider>` / `logout <provider>` / `status [<provider>]` operate on a named provider; `status` with no argument lists each `oauth:codex` provider's sign-in state.
- On upgrade the legacy global credential is **cleared** (no migration): each `oauth:codex` provider signs in fresh. This keeps the model purely per-provider with no global-fallback path.
- The interactive provider editor gains: a persistent key-hint line that action/status output can never cover (audited across every config TUI), editing an existing provider (api = prefilled field edit; oauth = a sign-in / sign-out / switch-account submenu), and running those OAuth actions by saving, leaving the full-screen TUI, performing the async sign-in/out for that provider, then reopening the editor.

## Non-Goals

- No migration of the existing global credential (decided: clear and re-login per provider).
- Multi-member / strategy model editing is unchanged; only the provider add/edit + credential flows change.
- Non-Codex OAuth types are out of scope (`oauth:codex` is the only OAuth provider type today).
- API-provider key storage is unchanged (still the `key` field in `providers.toml`).
- The remote `config provider edit` path's OAuth actions are out of scope for this change; the OAuth login/logout/switch actions target the local `fleety config` menu path first.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `codex-oauth`: sign-in / status / logout and the server token store become per-provider; the legacy global token is cleared on upgrade.
- `server-credential-store`: the credential frames carry a `provider`; the server keeps a Codex credential per provider name (not one global credential); a Codex credential frame without a provider is rejected; the credential capability version bumps to 3.
- `interactive-config-panel`: persistent nav hints on every screen, editing an existing provider, and OAuth sign-in / sign-out / switch-account for `oauth:codex` providers.

## Impact

- Affected specs: codex-oauth, server-credential-store, interactive-config-panel
- Affected code:
  - Modified:
    - crates/fleety-protocol/src/lib.rs
    - crates/fleety-tools/src/oauth.rs
    - crates/fleety-cli/src/auth.rs
    - crates/fleety-cli/src/main.rs
    - crates/fleety-cli/src/provider_tui.rs
    - crates/fleety-cli/src/config_panel.rs
    - crates/fleety-server/src/conn.rs
    - crates/fleety-server/src/providers.rs
    - crates/fleety-cli/tests/cli_smoke.rs
    - crates/fleety-daemon/tests/fleetyd_smoke.rs
    - README.md
    - docs/env.md
    - docs/design-cli-config.md
  - New:
    - (none)
  - Removed:
    - (none)
