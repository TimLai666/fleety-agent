## ADDED Requirements

### Requirement: The CLI manages a connected server's config over the connection

The CLI SHALL manage the connected server's configuration — flat `FLEETY_*` settings and the `provider`/`group`/`role` pool — over the existing client connection, without shell access to the server host. A `config` request SHALL carry a target; the default target is the connected server. The server SHALL execute the request against its own config file and provider file by reusing the shared config logic (the same code paths the local command uses), returning the result or an error message (never crashing). A mutating request SHALL be recorded in the audit log.

#### Scenario: set a server setting from the CLI

- **WHEN** `fleety config set FLEETY_MODEL gpt-5` runs with the default (server) target against a connected server
- **THEN** the server persists the change to its own config and returns a success result; no shell access to the server was needed

#### Scenario: manage the provider pool remotely

- **WHEN** a `provider`/`group`/`role` config request targets the server
- **THEN** the server applies it to its own `providers.toml` (reusing the same validation + atomic write as the local subcommands) and returns the result

#### Scenario: a bad request is reported, not crashed

- **WHEN** a config request names an unknown key or an invalid provider operation
- **THEN** the server returns an error result and its config files are unchanged

### Requirement: Remote config requires an authenticated connection

When the server requires auth, a config request over an unauthenticated connection SHALL be rejected with an unauthenticated error and SHALL NOT change any file. Config requests SHALL travel only over the same authenticated connection used for chat (token + pairing); the design recommends TLS for remote use.

#### Scenario: unauthenticated config is refused

- **WHEN** the server has auth required and receives a config request on a connection that has not authenticated
- **THEN** it returns an unauthenticated error and makes no change

### Requirement: Apply-time is reported honestly

A successful config change SHALL report when it takes effect, determined by the operation: a `providers.toml` change (a mutating `provider`/`group`/`role` op) takes effect on the next connection (the provider registry is rebuilt per connection from that file); a flat `set`/`unset` takes effect on restart (flat settings are seeded into the environment at boot and the environment takes precedence, so a file change is shadowed until restart). A read (`list`/`get`) reports no effect. The classification SHALL be a pure function of the operation. This change SHALL NOT claim mid-session hot-swap.

#### Scenario: a provider-pool change takes effect next connection

- **WHEN** a mutating `provider`/`group`/`role` op is applied on the server
- **THEN** the result reports it takes effect on the next connection

#### Scenario: a flat setting needs a restart

- **WHEN** a flat `set`/`unset` of a `FLEETY_*` setting is applied on the server
- **THEN** the result reports it takes effect after a server restart

##### Example: effect classification

| operation | effect |
|---|---|
| `provider add …` / `group set …` / `role set …` | next connection |
| `set FLEETY_MODEL gpt-5` / `unset FLEETY_ADDR` | restart |
| `set FLEETY_POLICY …` | restart |
| `list` / `get FLEETY_MODEL` / `provider list` | (none — read) |

### Requirement: Local target preserves existing behavior; device target is deferred

A config request with the `local` target SHALL edit the CLI host's own files exactly as today (no connection). A `device` target SHALL be rejected in this change with a message that names it as a follow-up, so nothing is silently ignored. Running the existing local `fleety-server config` on the server host SHALL remain available as a bootstrap path.

#### Scenario: local target is unchanged

- **WHEN** `fleety config --target local set …` runs
- **THEN** it edits the CLI host's own config files without using the connection, exactly as before this change

#### Scenario: device target is explicitly not yet supported

- **WHEN** a config request targets a device id
- **THEN** it is rejected with a message that this is a follow-up change (not silently ignored)
