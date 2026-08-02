## ADDED Requirements

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
