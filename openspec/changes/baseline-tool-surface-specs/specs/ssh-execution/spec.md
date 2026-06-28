## ADDED Requirements

### Requirement: Run commands on a remote host over SSH

The system SHALL provide an `ssh_exec` tool that runs a command on a remote `host` using the system ssh client in non-interactive BatchMode, with optional `user`, `port`, and `identity` (private-key path). The `host` argument SHALL be validated against option injection (a single token not starting with `-`). Irreversible remote commands SHALL be treated as critical.

#### Scenario: reject an injection-shaped host

- **WHEN** `ssh_exec` is called with a `host` beginning with `-`
- **THEN** the call is refused with an invalid-host error and no ssh process is spawned
