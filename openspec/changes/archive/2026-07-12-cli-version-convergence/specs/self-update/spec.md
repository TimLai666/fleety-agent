## ADDED Requirements

### Requirement: The CLI converges to the server's version on connect

When the fleety CLI connects to a server and the server's version (carried in the `Welcome` frame) is newer than the CLI's own version, the CLI SHALL update itself forward-only to the server's exact version and then re-execute the current command so it runs on the matching version. Convergence SHALL be forward-only: the CLI SHALL NOT downgrade when it is newer than, or equal to, the server, and SHALL do nothing when the server reports no version (an older server). Convergence SHALL be enabled by default and disableable via `FLEETY_CLI_AUTO_UPDATE`. It SHALL degrade gracefully: if the self-update cannot complete (permissions, network, or no matching artifact), the CLI SHALL warn and continue on its current version rather than failing the command. The re-execution SHALL be guarded so a single connect triggers at most one convergence attempt (no update loop).

#### Scenario: an older CLI converges to a newer server

- **WHEN** the CLI connects and the `Welcome` server version is newer than the CLI's version, with convergence enabled
- **THEN** the CLI updates itself to the server's exact version and re-executes the current command on the new version

#### Scenario: no downgrade and no-op cases

- **WHEN** the CLI's version is equal to or newer than the server's, or the server reports no version
- **THEN** the CLI does not update and runs the command unchanged

#### Scenario: convergence failure does not block the command

- **WHEN** the self-update cannot complete (e.g. the binary is not writable)
- **THEN** the CLI warns and continues on its current version instead of failing

#### Scenario: disabled by configuration

- **WHEN** `FLEETY_CLI_AUTO_UPDATE` is off
- **THEN** the CLI never self-updates on connect, regardless of the server version
