## ADDED Requirements

### Requirement: Daemon connection configuration

The daemon SHALL read `FLEETY_AGENT_URL` for the server WebSocket URL, trying mDNS for 2 seconds before falling back to `ws://127.0.0.1:8787`. It SHALL read `FLEETY_DEVICE_ID` for this device's id (default the hostname, falling back to `fleetyd-device`; the value is used verbatim and is not sanitized, so a path-safe id is the operator's responsibility) and `FLEETY_DEVICE_ROOT` for the filesystem root its on-device tools operate within (default the current working directory).

#### Scenario: URL falls back to localhost

- **WHEN** `FLEETY_AGENT_URL` is unset and mDNS finds nothing within 2 seconds
- **THEN** the daemon connects to `ws://127.0.0.1:8787`

### Requirement: Token pairing and persistence

`FLEETY_PAIRING_CODE` SHALL, when passed once, enroll a new device: the server mints a token in the `Welcome` message and the daemon writes it to `~/.fleety/fleetyd.token`. On later starts the daemon SHALL load that persisted token. `FLEETY_TOKEN` SHALL override the persisted token when set.

#### Scenario: pairing persists a minted token

- **WHEN** the daemon starts with `FLEETY_PAIRING_CODE` set and no stored token
- **THEN** it receives a minted token in `Welcome` and writes it to `~/.fleety/fleetyd.token` for reuse
