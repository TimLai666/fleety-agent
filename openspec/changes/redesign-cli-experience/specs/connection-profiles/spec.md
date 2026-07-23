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

mDNS TXT metadata SHALL be treated as an untrusted discovery hint, not as Server identity proof. Automatic mDNS resolution SHALL never attach a stored token, even when an advertised fingerprint equals a saved fingerprint. A credentialed profile SHALL NOT adopt or persist an automatically discovered endpoint change until the user explicitly reselects and re-pairs that Server. Explicit `connection set-url` and Settings edits SHALL persist the user-authored URL only after clearing the old token and fingerprint; they SHALL remain uncredentialed until re-pairing succeeds. Re-pairing is an explicit credential-recovery action, not cryptographic endpoint identity proof. Daemon pin, token-clear, and token-persist mutations SHALL target only the exact explicitly selected owner profile.

#### Scenario: current A cannot borrow pinned B

- **GIVEN** profile `A` is current without a URL and profile `B` has a pinned fingerprint and token
- **WHEN** automatic discovery sees `B`
- **THEN** it SHALL NOT send `B`'s token as `A`, pin `B` onto `A`, or mutate either profile's identity implicitly

#### Scenario: copied TXT fingerprint cannot receive a stored token

- **GIVEN** profile `A` has a stored token and fingerprint at endpoint `old`, and an mDNS advertiser at endpoint `new` copies that fingerprint
- **WHEN** automatic discovery or sticky recovery evaluates `new`
- **THEN** it SHALL NOT send `A`'s token to `new`, persist `new`, or report the profile healed; the user SHALL be directed to explicitly reselect and re-pair

### Requirement: Persisted profile switching reconnects the active Daemon

After `connection use` or an interactive persisted profile switch succeeds, the CLI SHALL notify the running local `fleetyd` through its owner control path. Each request SHALL remain durable through consumption until its terminal result is observed, SHALL NOT be silently overwritten by a later request, and SHALL receive exactly one durable success or failure settlement for its nonce; this does not promise exactly-once transport connection attempts. Success SHALL be settled only after the selected Server sends `Welcome`, authentication completes, its identity matches the saved pin, and the persisted owner snapshot still matches the resolved target at the atomic settlement boundary. Resolve, connect, `Hello`, authentication, identity, owner drift, stop, restart, and bounded-handshake failures SHALL settle failure. Notification failure SHALL preserve the saved profile but report a recoverable incomplete state rather than claiming the switch is fully active.

#### Scenario: A to B updates every live owner view

- **WHEN** the user changes current profile from `A` to `B`
- **THEN** CLI Server state, Server snapshot, Daemon snapshot, and the running `fleetyd` session SHALL all resolve to `B` before the workflow reports a fully refreshed state

#### Scenario: timed-out request is not overwritten

- **GIVEN** the Daemon is busy with an inline tool and has not consumed reconnect request `r1`
- **WHEN** the caller times out and another caller submits request `r2`
- **THEN** `r1` SHALL remain durable, `r2` SHALL be rejected as already pending, and the Daemon SHALL later settle `r1` exactly once

### Requirement: Profile switching consumes one live leased target snapshot

The profile URL, token, and fingerprint used for reconnect SHALL be read together inside the `connections.toml` mutation lease after the current-profile update. The reconnect SHALL NOT reuse credentials captured before that lease.

#### Scenario: concurrent credential rotation wins

- **GIVEN** profile `B`'s token or fingerprint rotates while a switch is waiting for the connection lease
- **WHEN** the switch acquires the lease
- **THEN** its reconnect SHALL use the latest complete `B` target snapshot

## MODIFIED Requirements

### Requirement: Sticky connections heal by fingerprint when the address moves

When connecting to a credentialed profile's saved URL fails, the client SHALL NOT treat an mDNS TXT fingerprint as identity proof, attach the stored token to a discovered endpoint, persist a discovered URL, or report the profile healed. The CLI one-shot path and Daemon reconnect loop SHALL preserve the saved profile and direct the user to explicitly reselect and re-pair. A successful saved connection SHALL proceed without a discovery scan. Transparent endpoint healing SHALL NOT return unless the transport supplies cryptographic Server identity proof.

#### Scenario: the server moves to a new IP

- **WHEN** the saved URL stops answering and a scan finds an advertiser with the pinned fingerprint at a new URL
- **THEN** the profile SHALL remain unchanged, no stored token SHALL be sent to the advertiser, and the user SHALL be directed to explicitly reselect and re-pair

##### Example: copied fingerprint at a new address

- **GIVEN** profile `office` stores URL `ws://10.0.0.2:8787`, token `old-token`, and fingerprint `server-a`
- **WHEN** `ws://10.0.0.9:8787` advertises TXT fingerprint `server-a` after the saved URL stops answering
- **THEN** `office` SHALL retain its original URL and credential, and the new endpoint SHALL receive neither `old-token` nor a healed status

#### Scenario: a different server on the LAN is never adopted

- **WHEN** the saved URL stops answering and a scan finds only advertisers with different or absent fingerprints
- **THEN** the profile SHALL remain unchanged, no stored token SHALL be sent to any advertiser, and the original failure plus explicit recovery guidance SHALL be reported

##### Example: unrelated advertiser

- **GIVEN** profile `office` is pinned to `server-a`
- **WHEN** the saved URL stops answering and discovery returns only `server-b`
- **THEN** `office` SHALL remain byte-identical and the failure SHALL direct `fleety --profile office pair <code>`

#### Scenario: healthy connections never scan

- **WHEN** the current profile's URL answers
- **THEN** no discovery scan SHALL run and the saved connection SHALL proceed

##### Example: saved endpoint remains reachable

- **GIVEN** profile `office` has a reachable saved URL
- **WHEN** a CLI one-shot command or Daemon reconnect uses `office`
- **THEN** it SHALL connect to the saved URL without starting mDNS discovery or changing the profile
