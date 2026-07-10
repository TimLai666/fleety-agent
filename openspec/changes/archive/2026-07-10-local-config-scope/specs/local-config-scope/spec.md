## ADDED Requirements

### Requirement: The local CLI config surface is scoped to this device's settings

`fleety config --target local` — its `list` and its interactive `edit` screen — SHALL show only the settings that affect this device's own `fleety` behavior (the `Cli` and `Shared` scopes), not `Server`/`Daemon`-scoped settings. Editing a non-local key through the local target (`get`/`set`/`unset` of a `Server` or `Daemon` setting) SHALL be refused with a message that points to the correct place to edit it (the server, via the default `fleety config`), rather than silently writing a value the local host never reads. The unfiltered dispatch is preserved for `fleety-server config` / `fleetyd config` on their own hosts (each edits its own scopes); only the CLI's local target is restricted.

#### Scenario: local list shows only Cli/Shared settings

- **WHEN** `fleety config --target local list` runs
- **THEN** it lists Cli/Shared settings (e.g. the voice + timezone settings) and does not list Server settings (e.g. `FLEETY_ADDR`, `FLEETY_POLICY`, `FLEETY_MODEL_KEY`)

#### Scenario: setting a server key locally is refused with direction

- **WHEN** `fleety config --target local set FLEETY_ADDR 0.0.0.0:8787` runs (a Server-scoped key)
- **THEN** it is refused with a message telling the user to set it on the server (via `fleety config set`, which targets the connected server), and nothing is written locally

#### Scenario: setting a local key still works

- **WHEN** `fleety config --target local set FLEETY_TZ Asia/Taipei` runs (a Shared-scoped key)
- **THEN** it is written to this device's config.toml as before
