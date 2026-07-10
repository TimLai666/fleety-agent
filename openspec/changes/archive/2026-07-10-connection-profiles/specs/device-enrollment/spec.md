## ADDED Requirements

### Requirement: Enrollment operates on connection profiles

`fleety init` and `fleety pair` SHALL operate on the connection profile store (`connections.toml`) rather than the flat `config.json` fields. `fleety init <url>` SHALL create or update a named profile (default name `default`) and make it current; `fleety pair <code>` SHALL pair the current profile and write the minted token into that profile. The device identity used during enrollment SHALL come from the shared `device_id` in `connections.toml`, and when migrating an existing device that `device_id` SHALL be preserved (locked to the pre-existing value), so enrollment on an already-known device does not change its identity.

#### Scenario: pairing writes the token into the current profile

- **WHEN** the user runs `fleety pair CODE` against an auth-required server
- **THEN** the minted token is stored on the current profile in `connections.toml`, and a later reconnect authenticates with that token

#### Scenario: enrollment keeps a migrated device's identity

- **WHEN** a device that previously enrolled (has a `device_id` in `config.json`) migrates and re-enrolls
- **THEN** its `device_id` is unchanged, so the server still recognizes it as the same device
