# authentication-default-on Specification

## Purpose

TBD - created by archiving change 'auth-default-on'. Update Purpose after archive.

## Requirements

### Requirement: Connection authentication is required by default

The server SHALL require a paired token to connect unless authentication is explicitly disabled. `FLEETY_REQUIRE_AUTH` SHALL default to on (`1`); the server treats any value other than an explicit `0` as "auth required". An operator who has set `FLEETY_REQUIRE_AUTH` to `0` or `1` (via env or config) keeps that behavior; only the previously-unset default changes from off to on.

#### Scenario: an unconfigured server requires auth

- **WHEN** the server starts with `FLEETY_REQUIRE_AUTH` unset anywhere
- **THEN** it requires a token to connect (auth is on by default)

#### Scenario: auth is still explicitly disable-able

- **WHEN** the server starts with `FLEETY_REQUIRE_AUTH=0`
- **THEN** it does not require a token to connect (the escape hatch is preserved)


<!-- @trace
source: auth-default-on
updated: 2026-07-10
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
-->

---
### Requirement: A fresh auth-required server guides first-device pairing

When the server starts with authentication required but has no way for any device to authenticate yet — no bootstrap `FLEETY_TOKEN` and no already-paired devices — it SHALL mint a short-lived pairing code at startup and surface it prominently (in the server log) together with the concrete `fleety pair <code>` next step, so a fresh secure server is pairable rather than an unreachable brick. When a bootstrap token exists or at least one device is already paired, no such code is minted (it is not a first run).

#### Scenario: first run mints and shows a pairing code

- **GIVEN** authentication is required, `FLEETY_TOKEN` is unset, and no device has paired
- **WHEN** the server starts
- **THEN** it mints a short-lived pairing code and logs it with the `fleety pair <code>` instruction

#### Scenario: an already-initialized server does not mint a code

- **GIVEN** authentication is required and a device is already paired (or a bootstrap token is set)
- **WHEN** the server starts
- **THEN** no first-run pairing code is minted


<!-- @trace
source: auth-default-on
updated: 2026-07-10
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
-->

---
### Requirement: Mutating remote config is refused when auth is disabled

A server running with authentication disabled accepts any connection, so it SHALL refuse mutating remote configuration frames (a `config` operation whose effect is a change — `set`/`unset`/`provider`/`model`), returning an error that tells the operator to enable authentication first. Read-only remote config frames (`list`/`get`) SHALL still be served. When authentication is enabled, remote config frames are handled as before (the connection is already authenticated at Hello).

#### Scenario: a wide-open server refuses a remote config change

- **GIVEN** the server runs with `FLEETY_REQUIRE_AUTH=0`
- **WHEN** a client sends a mutating config frame (e.g. `config set FLEETY_POLICY require_approval`)
- **THEN** the server returns an error result (nothing is written) telling the operator to enable auth first

#### Scenario: reads are still allowed on a wide-open server

- **GIVEN** the server runs with `FLEETY_REQUIRE_AUTH=0`
- **WHEN** a client sends a read-only config frame (`config list`)
- **THEN** the server returns the rendered settings as usual

<!-- @trace
source: auth-default-on
updated: 2026-07-10
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
-->

---
### Requirement: Loopback connections are trusted on the same host

A connection whose transport peer address is a loopback address (IPv4 `127.0.0.0/8` or IPv6 `::1`) SHALL be accepted without a token or pairing code even when authentication is required, because a same-host process can already read the server's token and config files — requiring a token adds friction, not security. The peer address SHALL be taken from the connection socket (via the server's connect-info), never from a request header or any client-supplied field that could be spoofed. `FLEETY_TRUST_LOOPBACK` SHALL gate this: any value other than `0` (including unset) trusts loopback; `0` requires authentication even on loopback (for multi-tenant hosts or a reverse proxy that forwards remote connections over loopback). A trusted-loopback acceptance SHALL be reflected back to the client in `Welcome` so it can skip pairing prompts. Non-loopback (LAN/remote) connections SHALL be authenticated exactly as before.

#### Scenario: same-host client connects without pairing

- **WHEN** a client connects from a loopback peer to an auth-required server with loopback trust enabled and presents no token
- **THEN** the server accepts the connection and marks it loopback-trusted in `Welcome`

#### Scenario: loopback trust can be disabled

- **WHEN** `FLEETY_TRUST_LOOPBACK=0` and a loopback client presents no token
- **THEN** the server rejects it exactly like any unauthenticated connection

#### Scenario: remote connections are unaffected

- **WHEN** a client connects from a non-loopback (LAN) address without a token
- **THEN** the server requires authentication as before, regardless of the loopback-trust setting

##### Example: peer-address classification

| Peer address     | Loopback? |
| ---------------- | --------- |
| 127.0.0.1        | yes       |
| 127.0.5.9        | yes       |
| ::1              | yes       |
| 192.168.1.10     | no        |
| 10.0.0.4         | no        |

<!-- @trace
source: local-server-trust
updated: 2026-07-12
code:
  - crates/fleety-server/src/http.rs
  - crates/fleety-server/src/conn.rs
  - scripts/install-server.sh
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->