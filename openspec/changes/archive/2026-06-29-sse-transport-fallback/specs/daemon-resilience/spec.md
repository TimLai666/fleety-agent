## MODIFIED Requirements

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
