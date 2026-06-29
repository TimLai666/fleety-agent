# daemon-resilience Specification

## Purpose

TBD - created by archiving change 'service-lifecycle'. Update Purpose after archive.

## Requirements

### Requirement: The daemon reconnects after disconnect or sleep

fleetyd SHALL keep trying to reconnect to the server after a dropped connection — including after the device sleeps and wakes — using exponential backoff with a cap and jitter, resetting the delay after a successful connection. On each connection attempt it SHALL first try WebSocket and, when the WebSocket connection or upgrade fails (or when the operator has forced SSE), fall back to the SSE+POST transport; WebSocket remains the preferred transport. It SHALL NOT prevent the device from sleeping, and it SHALL exit cleanly on Ctrl+C or a service stop.

#### Scenario: wake resumes the connection

- **WHEN** the device sleeps (dropping the connection) and later wakes with the network back
- **THEN** fleetyd reconnects automatically without manual intervention

#### Scenario: falls back to SSE when WebSocket is blocked

- **WHEN** a reconnect attempt's WebSocket connection or upgrade fails but plain HTTP to the server succeeds
- **THEN** fleetyd connects over the SSE+POST transport and resumes operating as a connected device

##### Example: backoff growth (base 1s, factor 2, cap 30s)

| Consecutive failures | Delay before next retry (before jitter) |
| -------------------- | --------------------------------------- |
| 1 | 1s |
| 2 | 2s |
| 3 | 4s |
| 6+ | 30s (capped) |
| after a success | reset to 1s |


<!-- @trace
source: sse-transport-fallback
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/Cargo.toml
  - README.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/http.rs
  - docs/env.md
-->

---
### Requirement: The daemon is single-instance

fleetyd SHALL avoid running two copies at once: the OS service manager keeps a second service copy from starting, and a pidfile guards against a manual launch alongside the service. A pidfile pointing at a live process SHALL cause a new launch to report "already running" and exit; a stale pidfile (dead pid) SHALL be ignored.

#### Scenario: second launch alongside the service bows out

- **WHEN** fleetyd is already running and another copy is launched
- **THEN** the second copy detects the live pidfile, reports "already running", and exits

<!-- @trace
source: service-lifecycle
updated: 2026-06-28
code:
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-server/src/winsvc.rs
-->