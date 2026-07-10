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