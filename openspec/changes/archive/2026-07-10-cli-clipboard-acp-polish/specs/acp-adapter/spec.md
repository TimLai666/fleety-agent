## ADDED Requirements

### Requirement: session/load returns a conformant ACP response

The adapter's `session/load` handler SHALL reply with a well-formed ACP `LoadSessionResponse` for the negotiated protocol version, sent after the mapped conversation's history is replayed as `session/update` notifications. The response SHALL be constructed from the ACP `LoadSessionResponse` shape the adapter targets rather than an arbitrary empty object, and SHALL be accepted by a conformant editor (verified end-to-end against Zed). A load failure SHALL return a JSON-RPC internal-error response and SHALL NOT crash the adapter.

#### Scenario: load replies with a conformant response

- **WHEN** the editor calls `session/load` for a known session and the history replay completes
- **THEN** the adapter returns a well-formed ACP `LoadSessionResponse` that a conformant editor accepts, after the `session/update` replay notifications

#### Scenario: load failure is a clean error

- **WHEN** the mapped server conversation cannot be resumed
- **THEN** the adapter returns a JSON-RPC internal-error response and keeps running, writing nothing non-protocol to stdout
