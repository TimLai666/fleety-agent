## MODIFIED Requirements

### Requirement: An interactive screen manages providers on a TTY

When stdout is a TTY, `config provider edit` SHALL open an interactive screen listing providers, groups, and roles, supporting add/edit/remove of a provider, setting a group's members and strategy, and binding a role. By default the screen SHALL edit the **connected server's** provider configuration: it is loaded from a structured config snapshot, edited in memory, and written back through a structured config apply under the snapshot's optimistic-lock revision — validation and the atomic write run on the server, with the same semantics as the local path. With an explicit `--target local`, the screen SHALL edit this host's own providers file exactly as before. Against a server that does not advertise credential-era protocol support (config protocol < 2), the remote screen SHALL refuse up front with an update-the-server error before opening — an older server would silently ignore the write-back. A validation failure SHALL be shown without writing; a concurrent-edit conflict SHALL be reported and the screen reloaded from a fresh snapshot rather than overwriting. Provider keys SHALL be masked in the display. When stdout is not a TTY, the system SHALL fall back to the subcommands.

#### Scenario: editing on a TTY saves through validation

- **WHEN** a provider is added in the interactive screen and saved
- **THEN** the configuration is validated and written atomically on the target host, and the key is masked on screen

#### Scenario: default target edits the connected server

- **WHEN** `config provider edit` runs on a TTY with no explicit target while connected to a remote server
- **THEN** the screen shows the server's providers, and saving updates the server's providers file — nothing is written on the CLI host

#### Scenario: explicit local target keeps the local file path

- **WHEN** `config --target local provider edit` runs on a TTY
- **THEN** the screen edits this host's own providers file with unchanged behavior

#### Scenario: old server is refused before the screen opens

- **WHEN** the remote screen is requested against a server advertising config protocol below 2
- **THEN** the command fails up front telling the user to update the server, and no editor opens

#### Scenario: concurrent edit surfaces as a conflict

- **WHEN** the server's configuration changed while the screen was open and the user saves
- **THEN** the save is rejected as a conflict and the screen reloads the current server state instead of overwriting

#### Scenario: non-TTY falls back

- **WHEN** `config provider edit` is invoked without a TTY
- **THEN** the interactive screen does not open and the subcommand path is used
