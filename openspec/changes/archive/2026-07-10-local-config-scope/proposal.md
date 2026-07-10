## Summary

Restrict the local CLI config surface (`fleety config --target local`, and its interactive edit screen) to the device's own `Cli`/`Shared` settings, so server-scoped keys are edited on the server (remotely), not misleadingly presented as locally editable.

## Motivation

`fleety config --target local list` and the interactive `config edit` screen currently show **every** registry setting regardless of scope — including `Server` keys like `FLEETY_ADDR`, `FLEETY_POLICY`, or `FLEETY_MODEL_KEY`. But `--target local` edits *this device's* `~/.fleety/config.toml`; a `Server` value written there does nothing (the server reads its own config), so the local surface invites a confusing no-op edit. The three-layer design (§3.2) makes "this device" the `Cli`/`Shared` layer and "the server" a separate remote layer; the local config command should reflect that boundary.

## Proposed Solution

- **Local list is scoped.** `fleety config --target local list` and the interactive `config edit` screen show only `Cli`/`Shared` settings — the ones that actually affect this device's `fleety` behavior (voice, transport, display tz, …).
- **Local get/set/unset of a server key is refused with direction.** `fleety config --target local set FLEETY_ADDR …` (a `Server` key) is rejected with a message pointing to the server path (`fleety config set FLEETY_ADDR …`, which targets the connected server) instead of silently writing a dead value.
- The shared `fleety_tools::config` dispatch gains a scope-filtered variant; `fleety-server` / `fleetyd` keep the unfiltered behavior on their own hosts (each edits its own scopes). Only the CLI's local path is restricted.

## Non-Goals (optional)

- Changing what `fleety-server config` / `fleetyd config` show on their own hosts (each already edits its own settings; not this change's concern).
- The remote server-config surface (default `fleety config`, which targets the server) — unchanged.
- The interactive all-in-one three-region panel (Phase 2 change `remote-config-panel`).

## Alternatives Considered (optional)

- **Hide server keys everywhere in the shared dispatch.** Rejected: `fleety-server config list` legitimately shows `Server` keys on the server host; the restriction is specific to the CLI's *local* target.

## Impact

- Affected specs: `local-config-scope` (new — the local CLI config surface is scoped to Cli/Shared)
- Affected code:
  - New: (none)
  - Modified: crates/fleety-tools/src/config.rs (scope-filtered rows + a scoped run + a same-scope guard), crates/fleety-cli/src/config.rs (local path uses the scoped run; interactive edit rows filtered to Cli/Shared), docs/roadmap.md, docs/STATUS.md
  - Removed: (none)
