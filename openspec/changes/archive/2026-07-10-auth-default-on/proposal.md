## Why

`FLEETY_REQUIRE_AUTH` defaults to `0` today — **anyone who can reach the server can use it, no pairing needed** (design §1/§4, a red-team hole). Cross-device Fleety is meant to be a trusted fleet: a connection should be authenticated by default, and the design's hard bottom line (§4, M1) is "authentication defaults on" plus "remote write ⇒ auth must be on". Without this, the upcoming remote-config surface would let an unauthenticated peer change a wide-open server's settings.

## What Changes

- **Authentication defaults on.** `FLEETY_REQUIRE_AUTH` defaults to `1`; the server requires a paired token to connect unless it is *explicitly* set to `0`. Existing env/config that sets it keeps working; only the unset default flips.
- **First-run pairing guidance.** When a server starts auth-required but has no way in yet (no bootstrap `FLEETY_TOKEN` and no paired devices), it mints a short-lived pairing code and logs it prominently with the exact `fleety pair <code>` next step — so a fresh secure server is not an unpairable brick.
- **Remote write ⇒ auth must be on.** A server running with auth disabled (`FLEETY_REQUIRE_AUTH=0`) refuses any *mutating* remote config frame (reads still allowed), with a message telling the operator to enable auth first — closing the "wide-open server, remotely reconfigured by anyone" hole.

## Non-Goals (optional)

- Sensitive-key overwrite warnings + audit, `wss`/TLS transport requirements, pairing-code hardening (longer codes, single active code, redeem rate-limit/lockout), and `ConfigSnapshot` sensitive-field tiering — the §4 "extra defenses", a separate hardening change.
- The `ConfigSnapshot`/`ConfigApply` wire protocol and the interactive all-in-one panel (Phase 2 change `remote-config-panel`).
- Owner/normal device tiering (design keeps it as a future advanced option).
- Changing the `FLEETY_ADDR` default (a separate, still-open decision).

## Capabilities

### New Capabilities

- `authentication-default-on`: connection auth is required by default, a fresh auth-required server guides first-device pairing, and mutating remote config is refused on an auth-disabled server.

### Modified Capabilities

(none)

## Impact

- Affected specs: `authentication-default-on` (new)
- Affected code:
  - New: (none)
  - Modified: crates/fleety-tools/src/config.rs (FLEETY_REQUIRE_AUTH default 0→1), crates/fleety-server/src/main.rs (require_auth default-on read + first-run pairing guidance), crates/fleety-server/src/auth.rs (uninitialized-store helper), crates/fleety-server/src/conn.rs (ConfigExec mutating-frame gate), docs/roadmap.md, docs/STATUS.md
  - Removed: (none)
