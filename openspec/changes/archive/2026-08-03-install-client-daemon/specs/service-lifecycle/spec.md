## ADDED Requirements

### Requirement: Client installation provisions the local daemon

The macOS/Linux and Windows client installers SHALL provision matching `fleety` and `fleetyd` binaries from the same release target before reporting a successful client installation. After both binaries are installed, the client installer SHALL register and start the local `fleetyd` service. The installer SHALL NOT enable login or boot autostart by default and SHALL NOT create or migrate connection credentials.

#### Scenario: Unix client installation starts the daemon

- **WHEN** the documented macOS or Linux client installer completes successfully
- **THEN** the selected install directory contains the matching `fleety` and `fleetyd` binaries, the daemon service is registered, and the daemon is started
- **AND** login or boot autostart remains disabled unless the user enables it separately

#### Scenario: Windows client installation requires elevation for service registration

- **WHEN** the documented Windows client installer installs both executables but service registration lacks Administrator rights
- **THEN** the installer exits non-zero with an actionable elevation message
- **AND** it does not report a successful client installation

##### Example: non-administrator service registration

- **GIVEN** `fleetyd-x86_64-pc-windows-msvc.zip` extracts both executables into the selected directory
- **WHEN** `fleetyd.exe install` returns an access-denied error
- **THEN** the installer exits non-zero and identifies Administrator rights as the required remediation

#### Scenario: daemon asset failure prevents false success

- **WHEN** the client installer cannot download, extract, or validate the matching `fleetyd` asset
- **THEN** it exits non-zero with a daemon-specific error
- **AND** it does not claim that installing only `fleety` completed the client bootstrap

#### Scenario: installation does not alter pairing state

- **WHEN** the client installer installs and starts `fleetyd` before the user pairs the device
- **THEN** it does not create, replace, or migrate connection credentials
- **AND** a later explicit `fleety init` or pairing flow remains responsible for enrollment

### Requirement: Release packaging exposes every client runtime asset

The release workflow SHALL verify that each supported target publishes non-empty `fleety`, `fleety-server`, and `fleetyd` artifacts in the archive format consumed by the corresponding installer before upload.

#### Scenario: missing daemon artifact fails the release package check

- **WHEN** a target package is built without a non-empty `fleetyd` archive
- **THEN** the release job fails before uploading the release assets

### Requirement: Server installation documents the client boundary

The server installer SHALL continue to support server-only deployment and SHALL identify the client installer as the path for provisioning `fleetyd` on a separate client device.

#### Scenario: server-only installation remains separate

- **WHEN** an operator runs the server installer
- **THEN** it installs the server deployment components without silently registering a client daemon service
- **AND** its closing guidance identifies the client installer for devices that need `fleetyd`
