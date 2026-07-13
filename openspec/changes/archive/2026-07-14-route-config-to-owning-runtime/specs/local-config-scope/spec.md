## MODIFIED Requirements

### Requirement: The local CLI config surface is scoped to this device's settings

fleety config --target cli and its compatibility alias --target local SHALL show and edit only settings owned by the fleety CLI, which is the Cli scope. Shared, Server, and Daemon settings SHALL be excluded. Editing a non-Cli key through the cli target SHALL be refused with a message that identifies the owning runtime. The unfiltered bootstrap dispatch is not exposed through fleety CLI; direct owner binaries SHALL enforce their own allowed scope sets.

#### Scenario: cli list shows only Cli settings

- **WHEN** fleety config --target cli list runs
- **THEN** it lists Cli settings such as FLEETY_VOICE_AUDIO and excludes Shared, Server, and Daemon settings

#### Scenario: setting a server key through cli is refused with direction

- **WHEN** fleety config --target cli set FLEETY_ADDR 0.0.0.0:8787 runs
- **THEN** it is refused with a message identifying the server owner and nothing is written locally

#### Scenario: setting a shared key through local alias is refused

- **WHEN** fleety config --target local set FLEETY_TZ Asia/Taipei runs
- **THEN** it is refused with a message identifying fleetyd as the owner and nothing is written by the CLI

#### Scenario: setting a Cli key still works

- **WHEN** fleety config --target cli set FLEETY_VOICE_AUDIO auto runs
- **THEN** the Cli-scoped value is persisted for the next fleety command
