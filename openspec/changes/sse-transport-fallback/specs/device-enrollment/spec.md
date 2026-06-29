## MODIFIED Requirements

### Requirement: Daemon connection configuration

The daemon SHALL read `FLEETY_AGENT_URL` for the server URL, trying mDNS for 2 seconds before falling back to `ws://127.0.0.1:8787`. From the resolved server host the daemon SHALL derive both the WebSocket endpoint and the HTTP(S) endpoints used by the SSE+POST fallback, so that one configured host serves both transports. The daemon SHALL read a setting to force the SSE+POST transport and a setting to disable the SSE fallback; when neither is set, it tries WebSocket first and falls back to SSE. It SHALL read `FLEETY_DEVICE_ID` for this device's id (default the hostname, falling back to `fleetyd-device`; the value is used verbatim and is not sanitized, so a path-safe id is the operator's responsibility) and `FLEETY_DEVICE_ROOT` for the filesystem root its on-device tools operate within (default the current working directory).

#### Scenario: URL falls back to localhost

- **WHEN** `FLEETY_AGENT_URL` is unset and mDNS finds nothing within 2 seconds
- **THEN** the daemon connects to `ws://127.0.0.1:8787`

#### Scenario: SSE endpoint derived from the same host

- **WHEN** the daemon has resolved a server host and the WebSocket transport is unavailable
- **THEN** it reaches the SSE and POST endpoints on that same host without requiring a separately configured URL
