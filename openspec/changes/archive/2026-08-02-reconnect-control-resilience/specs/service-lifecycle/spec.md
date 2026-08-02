## ADDED Requirements

### Requirement: Reconnect control ownership supports safe stale recovery

The Daemon's reconnect control directory SHALL expose read-only inspection of the recorded process identity, process-start identity, owner generation, active request nonce, and age. Destructive stale-control recovery SHALL require evidence that the recorded owner is no longer live or an explicit operator confirmation after inspection. Recovery SHALL reject a live owner, a mismatched process-start identity, a successor lock, or a control-version mismatch.

#### Scenario: inspection is read-only

- **WHEN** an operator inspects reconnect control ownership
- **THEN** the command SHALL report ownership evidence without deleting, rewriting, or reaping any control file

##### Example:

- **GIVEN** the control record contains PID `123`, process-start `start-a`, and owner generation `g-7`
- **WHEN** the operator runs control inspection
- **THEN** the output SHALL show those values and the journal, receipt, and lock files SHALL retain their original bytes

#### Scenario: a dead owner can be recovered explicitly

- **GIVEN** the recorded process is dead and no successor lock owns the control directory
- **WHEN** the operator confirms stale-control recovery
- **THEN** the recovery SHALL remove only the stale control artifacts and SHALL report the recovered owner evidence

#### Scenario: a live or reused process is protected

- **GIVEN** the recorded PID is live but its process-start identity differs, or a successor lock is present
- **WHEN** an operator requests stale-control recovery
- **THEN** the operation SHALL refuse cleanup and SHALL preserve every control artifact

### Requirement: Terminal reconnect records follow an explicit retention policy

The Daemon SHALL retain terminal reconnect receipts and success proofs for a documented duration. Retention cleanup SHALL run under the same ownership and control-version checks as other control mutations and SHALL never remove an active journal, a retained success proof, or a record that is not yet eligible for expiry.

#### Scenario: active requests outlive retention cleanup

- **GIVEN** an active reconnect request exists when retention cleanup runs
- **WHEN** cleanup evaluates the control directory
- **THEN** the active journal SHALL remain and the request SHALL remain queryable

#### Scenario: expired terminal records are reaped together

- **GIVEN** a terminal record and its associated proof or receipt have passed the retention deadline
- **WHEN** an authorized retention cleanup runs
- **THEN** cleanup SHALL reap only that complete eligible record and SHALL leave unrelated requests intact

#### Scenario: cleanup refuses ambiguous ownership

- **GIVEN** the control owner or control identity cannot be proven
- **WHEN** retention cleanup runs
- **THEN** it SHALL report a non-success state and SHALL preserve the affected record
