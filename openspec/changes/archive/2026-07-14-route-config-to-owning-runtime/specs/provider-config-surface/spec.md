## MODIFIED Requirements

### Requirement: An interactive screen manages providers on a TTY

When stdout is a TTY, config provider edit SHALL open an interactive screen listing providers, groups, and roles, supporting add, edit, and remove of a provider, setting a group's members and strategy, and binding a role. The screen SHALL always edit the connected server's provider configuration: it is loaded from a structured config snapshot, edited in memory, and written back through structured config apply under the snapshot revision. Validation and atomic persistence SHALL run on the server. An explicit cli or local target SHALL be rejected before the editor opens. Against a server below config protocol 2, the screen SHALL fail before opening with an update instruction. A validation failure SHALL be shown without writing. A concurrent-edit conflict SHALL be reported and the screen SHALL reload from a fresh snapshot rather than overwrite. Provider keys SHALL be masked. Without a TTY, the system SHALL use provider subcommands, which also target the server.

#### Scenario: editing on a TTY saves through server validation

- **WHEN** a provider is added in the interactive screen and saved
- **THEN** the configuration is validated and written atomically by the connected server and the key is masked on screen

#### Scenario: default target edits the connected server

- **WHEN** config provider edit runs on a TTY with no explicit target
- **THEN** the screen shows the server providers, saving updates the server providers file, and nothing is written on the CLI host

#### Scenario: explicit local target is rejected

- **WHEN** config --target local provider edit runs on a TTY
- **THEN** the command fails before the editor opens and directs the user to the connected server flow

#### Scenario: old server is refused before the screen opens

- **WHEN** the remote screen is requested against a server advertising config protocol below 2
- **THEN** the command fails with an update instruction and no editor opens

#### Scenario: concurrent edit surfaces as a conflict

- **WHEN** the server configuration changes while the screen is open and the user saves
- **THEN** the save is rejected as a conflict and the screen reloads current server state instead of overwriting

#### Scenario: non-TTY uses server subcommands

- **WHEN** config provider edit is invoked without a TTY
- **THEN** the interactive screen does not open and the server-targeted subcommand path is used
