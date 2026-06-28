## ADDED Requirements

### Requirement: The daemon reconnects after disconnect or sleep

fleetyd SHALL keep trying to reconnect to the server after a dropped connection — including after the device sleeps and wakes — using exponential backoff with a cap and jitter, resetting the delay after a successful connection. It SHALL NOT prevent the device from sleeping, and it SHALL exit cleanly on Ctrl+C or a service stop.

#### Scenario: wake resumes the connection

- **WHEN** the device sleeps (dropping the connection) and later wakes with the network back
- **THEN** fleetyd reconnects automatically without manual intervention

##### Example: backoff growth (base 1s, factor 2, cap 30s)

| Consecutive failures | Delay before next retry (before jitter) |
| -------------------- | --------------------------------------- |
| 1 | 1s |
| 2 | 2s |
| 3 | 4s |
| 6+ | 30s (capped) |
| after a success | reset to 1s |

### Requirement: The daemon is single-instance

fleetyd SHALL avoid running two copies at once: the OS service manager keeps a second service copy from starting, and a pidfile guards against a manual launch alongside the service. A pidfile pointing at a live process SHALL cause a new launch to report "already running" and exit; a stale pidfile (dead pid) SHALL be ignored.

#### Scenario: second launch alongside the service bows out

- **WHEN** fleetyd is already running and another copy is launched
- **THEN** the second copy detects the live pidfile, reports "already running", and exits
