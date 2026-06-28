## ADDED Requirements

### Requirement: Settings are discoverable and editable from the terminal

The CLI SHALL provide `config list`, `config get <key>`, `config set <key> <value>`, and `config unset <key>` commands over a typed registry of known settings. `list` SHALL show every known setting with its scope, current resolved value, and the value's source; `set` SHALL validate the key against the registry and persist it; `get` SHALL show the resolved value and source; `unset` SHALL remove the stored value. Unknown keys SHALL be rejected with an actionable message.

#### Scenario: list shows settings and sources

- **WHEN** the user runs `fleety config list`
- **THEN** each known setting is shown with its scope, current value, and whether that value came from the environment, the config file, or the default

#### Scenario: set then get round-trips

- **WHEN** the user runs `config set` for a known key and then `config get` for it
- **THEN** the value set is stored and reported back (with source = config) on get

#### Scenario: unknown key is rejected

- **WHEN** the user runs `config set` or `config get` for a key not in the registry
- **THEN** the command fails with an actionable message naming the unknown key and does not write anything

### Requirement: Settings persist to a config file consumed at boot

Stored settings SHALL be persisted to a config file (`~/.fleety/config.toml`, overridable) sectioned by scope, and the server and daemon SHALL honor those values at startup. A missing or corrupt config file SHALL degrade to environment/defaults without crashing.

#### Scenario: server honors a configured setting at boot

- **WHEN** a setting's environment variable is unset but it is present in the config file, and the server starts
- **THEN** the server uses the config-file value

#### Scenario: corrupt config does not crash

- **WHEN** the config file is unreadable or not valid TOML
- **THEN** the binary warns and continues using environment variables and defaults

### Requirement: Read precedence is environment, then config, then default

For any setting, the resolved value SHALL be the explicit environment variable when set and non-empty, otherwise the config-file value for that setting's scope, otherwise the registry default. An explicit environment variable SHALL always take precedence over the config file.

##### Example: resolution precedence

| Env set? | In config? | Resolved source |
| -------- | ---------- | --------------- |
| yes | yes | environment |
| yes | no | environment |
| no | yes | config |
| no | no | default |

#### Scenario: environment overrides config

- **WHEN** a setting is present in both the environment and the config file
- **THEN** the environment value is used and reported as the environment source

### Requirement: Interactive settings editor

The CLI SHALL provide an interactive settings screen (`fleety config edit`) that lists settings by scope and edits a selected setting's value through the same validated path as `config set`. Secret-flagged settings SHALL be masked in list views and revealed only while their field is being edited.

#### Scenario: editing a setting persists it

- **WHEN** the user opens the interactive editor, changes a setting, and saves
- **THEN** the new value is validated and written to the config file, the same as `config set`

#### Scenario: secrets are masked

- **WHEN** the interactive editor or `config list` displays a secret-flagged setting
- **THEN** its value is masked except while that specific field is being edited
