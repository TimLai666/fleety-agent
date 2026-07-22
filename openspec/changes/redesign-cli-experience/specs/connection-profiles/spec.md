## ADDED Requirements

### Requirement: Connection is the canonical profile command group

The CLI SHALL expose `connection` as the canonical command group for named Server profiles. The existing `server` group SHALL remain an alias that maps to the same parsed command and persistence implementation.

#### Scenario: legacy server command remains compatible

- **WHEN** the user runs `fleety server list`
- **THEN** it SHALL return the same profiles, ordering, current marker, and exit status as `fleety connection list`

### Requirement: Interactive profile switching is explicit and observable

The workspace SHALL provide a profile picker that shows name, label, endpoint, current marker, reachability when known, and transient override state. Selecting a profile SHALL not enable remote actions until connection and snapshot refresh have completed.

#### Scenario: profile becomes usable only after refresh

- **WHEN** the user selects a reachable profile
- **THEN** the workspace SHALL show Connecting, then load Server and Daemon owner states, and only then enable their mutations

##### Example: switch from A to B

- **GIVEN** profile `A` is connected and profile `B` is reachable
- **WHEN** the user selects `B`
- **THEN** remote Apply actions stay disabled until `B` authenticates and both Server and Daemon snapshots have replaced all `A` revisions

### Requirement: Automatic discovery never borrows another profile's identity

mDNS resolution SHALL use only the current profile's own fingerprint and token. A discovered Server owned by another saved profile SHALL require explicit selection before any stored credential is sent. Unowned discovery SHALL remain uncredentialed until pairing, and Daemon pin, heal, token-clear, and token-persist mutations SHALL target only the exact resolved owner profile.

#### Scenario: current A cannot borrow pinned B

- **GIVEN** profile `A` is current without a URL and profile `B` has a pinned fingerprint and token
- **WHEN** automatic discovery sees `B`
- **THEN** it SHALL NOT send `B`'s token as `A`, pin `B` onto `A`, or mutate either profile's identity implicitly

### Requirement: Persisted profile switching reconnects the active Daemon

After `connection use` or an interactive persisted profile switch succeeds, the CLI SHALL notify the running local `fleetyd` through its owner control path. The Daemon SHALL close its old Server session, resolve the newly current profile, reconnect immediately, and acknowledge the request. Notification failure SHALL preserve the saved profile but report a recoverable incomplete state rather than claiming the switch is fully active.

#### Scenario: A to B updates every live owner view

- **WHEN** the user changes current profile from `A` to `B`
- **THEN** CLI Server state, Server snapshot, Daemon snapshot, and the running `fleetyd` session SHALL all resolve to `B` before the workflow reports a fully refreshed state

### Requirement: Profile switching consumes one live leased target snapshot

The profile URL, token, and fingerprint used for reconnect SHALL be read together inside the `connections.toml` mutation lease after the current-profile update. The reconnect SHALL NOT reuse credentials captured before that lease.

#### Scenario: concurrent credential rotation wins

- **GIVEN** profile `B`'s token or fingerprint rotates while a switch is waiting for the connection lease
- **WHEN** the switch acquires the lease
- **THEN** its reconnect SHALL use the latest complete `B` target snapshot
