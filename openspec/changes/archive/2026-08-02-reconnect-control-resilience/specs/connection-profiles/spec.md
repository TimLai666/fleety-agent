## ADDED Requirements

### Requirement: Durable reconnect requests expose one nonce-addressed lifecycle

The existing reconnect journal SHALL reconstruct one lifecycle for every nonce, including submitted, claimed, in-progress, settled, cancelled, superseded, expired, and ambiguous states where applicable. A status operation SHALL return the nonce, profile, owner identity, lifecycle state, timestamps, replacement nonce, and retained terminal result when present. A terminal receipt SHALL remain authoritative for its documented retention period.

#### Scenario: status is repeatable

- **GIVEN** a reconnect request has a durable journal record
- **WHEN** an operator requests its status multiple times
- **THEN** every response SHALL describe the same durable state and SHALL not mutate the journal or extend retention

#### Scenario: duplicate nonces do not overwrite results

- **GIVEN** a nonce is active or has a retained terminal receipt
- **WHEN** another caller submits the same nonce
- **THEN** the new submission SHALL be rejected without replacing the existing journal, receipt, or success proof

#### Scenario: restart preserves the lifecycle

- **GIVEN** the Daemon stops after recording a request or terminal result
- **WHEN** the Daemon starts again and reads the control directory
- **THEN** it SHALL reconstruct the same request state or an explicit ambiguous state and SHALL not invent a conflicting terminal result

### Requirement: Reconnect cancellation and supersession are owner-safe

Cancellation SHALL be accepted only from the current control owner and before a terminal authenticated success proof exists. Supersession SHALL record the replacement nonce and settle the old request before the replacement takes ownership. A foreign owner, stale owner, live successor, or mismatched control identity SHALL receive a refusal and SHALL not delete or rewrite another request's durable evidence.

#### Scenario: the owner cancels before success

- **GIVEN** the current owner has a pending reconnect without a durable authenticated success proof
- **WHEN** the owner requests cancellation for that nonce
- **THEN** the journal SHALL record a terminal cancelled state and the Daemon SHALL stop attempting that request

#### Scenario: cancellation cannot erase success

- **GIVEN** the nonce has a durable authenticated success proof
- **WHEN** an operator requests cancellation
- **THEN** the operation SHALL be rejected and SHALL preserve the success proof and terminal success result

#### Scenario: supersession is ordered

- **GIVEN** an active request is owned by the current control instance
- **WHEN** the owner requests supersession with a new nonce
- **THEN** the old request SHALL become terminal superseded before the new request is accepted as its replacement
