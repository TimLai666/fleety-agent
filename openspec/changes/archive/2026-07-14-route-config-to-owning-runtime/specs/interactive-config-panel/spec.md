## MODIFIED Requirements

### Requirement: Bare fleety config opens a three-region interactive panel

On a TTY, fleety config with no arguments SHALL open a single interactive panel with four regions: Connection, CLI, Daemon, and Server. The Connection region manages connection profiles. The CLI region edits only Cli-scoped settings. The Daemon region loads and applies Daemon and Shared settings through fleetyd. The Server region loads and applies Server settings through fleety-server. Without a TTY, fleety config SHALL use the non-interactive text command path.

#### Scenario: the panel exposes all four owners from one entry

- **WHEN** fleety config runs on a TTY
- **THEN** a panel opens with Connection, CLI, Daemon, and Server regions and switching regions needs no target flag

#### Scenario: no TTY uses text commands

- **WHEN** fleety config list runs without a TTY
- **THEN** it uses the non-interactive text command path

## ADDED Requirements

### Requirement: Daemon and server regions persist only through their owners

The Daemon and Server panel regions SHALL keep independent availability, revision, snapshot entries, staged changes, and apply targets. A daemon edit SHALL be sent to fleetyd and a server edit SHALL be sent to fleety-server. If an owner is unavailable, its region SHALL display an unavailable state and SHALL NOT offer a direct-file fallback.

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
