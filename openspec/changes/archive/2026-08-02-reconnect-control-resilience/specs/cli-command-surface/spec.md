## ADDED Requirements

### Requirement: The CLI exposes nonce-addressed reconnect lifecycle operations

The CLI SHALL expose stable operations to inspect reconnect status, cancel an owned request, supersede an owned request with a replacement nonce, inspect reconnect control ownership, and request safe stale-control recovery. Each operation SHALL address the relevant nonce or control instance, SHALL return a distinct success or refusal class, and SHALL provide the next safe action for an ambiguous or expired result.

#### Scenario: status identifies a request

- **WHEN** an operator requests reconnect status for a nonce
- **THEN** the CLI SHALL report its lifecycle state, profile, owner context, timestamps, retained result, and replacement nonce when present

##### Example:

- **GIVEN** nonce `n-42` is terminally cancelled for profile `home`
- **WHEN** the operator requests status for `n-42`
- **THEN** the CLI output SHALL include `n-42`, `cancelled`, `home`, the terminal timestamp, and no replacement nonce

#### Scenario: cancellation reports refusal precisely

- **GIVEN** a nonce is settled successfully, owned by another process, or already cancelled
- **WHEN** the operator requests cancellation
- **THEN** the CLI SHALL return a non-success class that identifies the refusal reason and SHALL not claim that cancellation succeeded

#### Scenario: supersession exposes the replacement

- **WHEN** the current owner supersedes an active reconnect with a replacement nonce
- **THEN** the CLI SHALL report both nonce values and SHALL not report the replacement as accepted until the old request is durably settled as superseded

##### Example:

- **GIVEN** active nonce `n-old` is replaced by `n-new`
- **WHEN** supersession completes
- **THEN** the CLI SHALL report `n-old` as `superseded` with replacement `n-new` before reporting `n-new` as accepted

#### Scenario: control recovery requires inspection evidence

- **WHEN** an operator requests stale-control recovery without a dead-owner proof or explicit confirmation
- **THEN** the CLI SHALL refuse the destructive action and SHALL direct the operator to inspect ownership first

##### Example:

- **GIVEN** the control record reports PID `123` but no process-start proof is available
- **WHEN** the operator requests recovery without an explicit confirmation
- **THEN** the CLI SHALL return a refusal class and SHALL leave the control directory unchanged
