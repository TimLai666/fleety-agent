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

---
### Requirement: Owner-requested reconnects use one caller and sweep budget

The CLI wait and the Daemon's complete owner-requested candidate sweep SHALL derive from one documented budget. The sweep deadline SHALL fit within the caller timeout with reserved margin for durable settlement and response delivery. Candidate shares SHALL use the remaining whole-sweep deadline, and the ordinary non-reconnect endpoint budget SHALL remain independent.

#### Scenario: the caller covers the complete sweep

- **GIVEN** an owner-requested reconnect has multiple candidate endpoints
- **WHEN** the Daemon attempts the candidates
- **THEN** the caller wait SHALL remain open through the complete sweep and the reserved settlement margin

#### Scenario: a silent candidate does not consume the whole sweep

- **GIVEN** the first candidate accepts a transport but does not complete its handshake
- **WHEN** its candidate share expires
- **THEN** the Daemon SHALL release that candidate's share and SHALL attempt later candidates before settling failure

#### Scenario: an ordinary connection keeps its independent budget

- **WHEN** the Daemon connects without an owner-requested reconnect
- **THEN** it SHALL retain the ordinary endpoint budget and SHALL not apply the tighter owner-requested sweep budget

##### Example:

- **GIVEN** `fleetyd` starts without a pending reconnect nonce
- **WHEN** an endpoint requires more than the owner-requested share but less than `15 seconds`
- **THEN** the ordinary connection SHALL continue under its independent endpoint budget


<!-- @trace
source: reconnect-control-resilience
updated: 2026-08-02
code:
  - crates/fleety-cli/src/acp.rs
  - crates/agent-core/src/error.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/connection.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/main.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/server.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Reconnect housekeeping is bounded and restart-safe

Journal append, terminal receipt and success-proof publication, quarantine, cleanup, directory synchronization, and shutdown settlement SHALL use bounded retries. A failed housekeeping operation SHALL retain its durable evidence, return an actionable non-success state, and SHALL not hold a global reconnect lease or stop ordinary authenticated Daemon service indefinitely.

#### Scenario: a transient append failure remains visible

- **GIVEN** a reconnect settlement cannot be appended because the filesystem is temporarily unavailable
- **WHEN** the bounded retry policy expires
- **THEN** the request SHALL remain visible as unsettled or failed-to-persist and the Daemon SHALL continue ordinary service when its authenticated session remains usable

#### Scenario: ambiguous success proof is never silently removed

- **GIVEN** a success proof cannot be quarantined or synchronized durably
- **WHEN** the retry policy expires
- **THEN** the proof SHALL remain available for diagnosis and the operation SHALL not publish a conflicting failure or remove another request's proof

#### Scenario: restart converges after interruption

- **GIVEN** the Daemon stops during journal, receipt, proof, or cleanup handling
- **WHEN** it starts again
- **THEN** repeated recovery SHALL converge to the existing durable state without duplicate terminal results or an infinite retry loop

<!-- @trace
source: reconnect-control-resilience
updated: 2026-08-02
code:
  - crates/fleety-cli/src/acp.rs
  - crates/agent-core/src/error.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/connection.rs
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/main.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/server.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->