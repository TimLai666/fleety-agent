## MODIFIED Requirements

### Requirement: Daemon and server regions persist only through their owners

The Daemon and Server panel regions SHALL keep independent availability, revision, snapshot entries, staged changes, and apply targets. A daemon edit SHALL be sent to fleetyd and a server edit SHALL be sent to fleety-server. If an owner is unavailable, its region SHALL display an unavailable state and SHALL NOT offer a direct-file fallback. After the user saves a different current connection profile, the panel SHALL close the previous connection, discard both remote regions' prior snapshot, revision, and staged changes, connect using the newly selected profile, and reload the Server and current-device Daemon snapshots before either region can apply a change. A failed reconnect SHALL leave both remote regions unavailable and SHALL NOT restore or reuse the previous connection.

#### Scenario: daemon unavailable leaves other regions usable

- **GIVEN** the server connection works but fleetyd for the current device is not connected
- **WHEN** the panel opens
- **THEN** Connection, CLI, and Server remain usable while Daemon is marked unavailable

#### Scenario: server unavailable does not convert remote edits to local writes

- **GIVEN** the server cannot be reached
- **WHEN** the panel opens
- **THEN** Connection and CLI remain usable, Daemon and Server are marked unavailable, and no remote setting is written locally

#### Scenario: staged changes remain separated

- **WHEN** the user stages one daemon setting and one server setting
- **THEN** applying in either region sends only that region's changes and revision to its owner

#### Scenario: saved profile switch reconnects before remote use

- **GIVEN** the panel is connected to server B and profile A identifies a different server
- **WHEN** the user selects profile A as current and saves the Connection region
- **THEN** the panel closes the B connection, connects using profile A, and reloads A's Server and current-device Daemon snapshots before enabling either remote apply action

#### Scenario: old remote state is not carried to the new server

- **GIVEN** the panel has snapshot entries, revisions, and staged changes from server B
- **WHEN** the user saves profile A as current
- **THEN** all B-derived Server and Daemon state is discarded and no B-derived change can be sent through the A connection

#### Scenario: reconnect failure cannot fall back to the old server

- **GIVEN** the panel is connected to server B and the newly saved profile A cannot complete its connection and Hello handshake
- **WHEN** the profile switch is attempted
- **THEN** the B connection remains closed, Server and Daemon are unavailable, Connection and CLI remain usable, and no remote config file is modified

#### Scenario: daemon refresh failure does not hide a usable server

- **GIVEN** profile A connects and returns a Server snapshot but the current device daemon is unavailable on A
- **WHEN** the panel refreshes both remote regions
- **THEN** the Server region is usable with A's state and the Daemon region is unavailable
