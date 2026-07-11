# ws-liveness Specification

## Purpose

TBD - created by archiving change 'ws-liveness'. Update Purpose after archive.

## Requirements

### Requirement: Server sends WebSocket keepalive pings and reclaims half-open connections

The server SHALL send a WebSocket Ping control frame on every WebSocket connection at a configurable interval (`FLEETY_WS_PING_SECS`, default 20 seconds). The server SHALL treat any inbound frame — text, pong, or other control frames — as proof of liveness, and SHALL close a connection that produces no inbound frame within the liveness deadline (`FLEETY_WS_TIMEOUT_SECS`, default 60 seconds). Closing via the liveness deadline SHALL run the same disconnect cleanup as any other connection end (connection registry and advertised-tools entries removed), so routing to that device fails fast afterwards. A failed ping write SHALL end the connection immediately without waiting for the deadline. Non-positive or non-numeric configuration values SHALL fall back to the defaults.

#### Scenario: idle healthy connection survives indefinitely

- **WHEN** a WebSocket client stays connected with no message traffic for longer than the liveness deadline, and its WebSocket layer answers the server's pings
- **THEN** the server keeps the connection registered and routable across arbitrarily many ping intervals

#### Scenario: half-open connection is reclaimed and routing fails fast

- **WHEN** a device's WebSocket connection goes silent without closing (no reads, no writes, no close frame — e.g. sleep, NAT idle drop, network switch)
- **THEN** within the liveness deadline plus one ping interval the server closes the connection and removes the device from the connection registry, and a subsequent tool route to that device fails immediately with a not-connected error instead of waiting out the per-call timeout

##### Example: timing configuration parsing

| FLEETY_WS_PING_SECS | FLEETY_WS_TIMEOUT_SECS | Effective ping / deadline | Notes |
| ------------------- | ---------------------- | ------------------------- | ----- |
| (unset) | (unset) | 20s / 60s | defaults |
| 10 | 30 | 10s / 30s | explicit override |
| 0 | -5 | 20s / 60s | non-positive values fall back to defaults |
| abc | (unset) | 20s / 60s | non-numeric value falls back to default |


<!-- @trace
source: ws-liveness
updated: 2026-07-11
code:
  - .github/workflows/release.yml
  - crates/fleety-server/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/http.rs
  - docs/env.md
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-tools/src/deps/insyra.rs
-->

---
### Requirement: Clients detect half-open WebSocket links with a ping-adaptive read deadline

The shared client transport (used by both the CLI and fleetyd) SHALL arm a WebSocket read deadline (`FLEETY_WS_TIMEOUT_SECS`, default 60 seconds) only after it observes the first Ping frame on the current connection. Once armed, the absence of any inbound frame within the deadline SHALL be reported to the caller as the connection ending — the same observable shape as a closed link — so fleetyd runs its existing backoff reconnect and the CLI runs its existing link-closed handling. On a connection where no Ping is ever observed (an older server), the transport SHALL NOT arm the deadline and SHALL behave exactly as it did before this capability.

#### Scenario: armed deadline detects a dead link

- **WHEN** the server has pinged at least once on the connection and then no frame of any kind arrives within the read deadline
- **THEN** the client transport reports the connection as ended and the caller's existing reconnect or link-closed handling runs

#### Scenario: never-pinged connection is unaffected

- **WHEN** a client holds an idle connection to a server that never sends Ping frames
- **THEN** no read deadline is armed and the connection is not reported as ended, no matter how long it stays idle


<!-- @trace
source: ws-liveness
updated: 2026-07-11
code:
  - .github/workflows/release.yml
  - crates/fleety-server/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/http.rs
  - docs/env.md
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-tools/src/deps/insyra.rs
-->

---
### Requirement: Liveness uses WebSocket control frames without protocol changes

WebSocket liveness SHALL be implemented with WebSocket Ping/Pong control frames only, beneath the transport-agnostic connection loop. The `ClientMsg`/`ServerMsg` message set, their serialized shapes, and the protocol version SHALL be unchanged. The SSE+POST fallback transport's keepalive and timeout behavior SHALL be unchanged by this capability.

#### Scenario: old daemon is protected without an upgrade

- **WHEN** a fleetyd built before this capability connects to a server that sends keepalive pings
- **THEN** the daemon's WebSocket layer answers the pings automatically and the server's half-open detection works against it with no daemon-side change

#### Scenario: SSE fallback behavior is unchanged

- **WHEN** a client connects over the SSE+POST fallback transport
- **THEN** its keepalive and half-open detection behave exactly as they did before this capability

<!-- @trace
source: ws-liveness
updated: 2026-07-11
code:
  - .github/workflows/release.yml
  - crates/fleety-server/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/http.rs
  - docs/env.md
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-tools/src/deps/insyra.rs
-->