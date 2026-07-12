## ADDED Requirements

### Requirement: The TUI surfaces an authentication rejection instead of reconnecting forever

When the server rejects the connection with an authentication error (error kind `unauthenticated`), the TUI SHALL treat it as terminal: it SHALL show an actionable message (this device is not paired with the server; run `fleety pair <code>`, and a code can be minted with `fleety pair-code` on the server host) and stop, rather than treating the closed link as a transient drop and reconnecting. Other errors and ordinary dropped links SHALL keep the existing capped-backoff reconnect behavior unchanged. The classification of an error as an authentication rejection SHALL be a pure check on the error kind.

#### Scenario: unpaired TUI stops with guidance

- **WHEN** the TUI connects to an auth-required server without a valid token and the server rejects it as `unauthenticated`
- **THEN** the TUI shows the not-paired guidance and exits instead of reconnecting

#### Scenario: a transient drop still reconnects

- **WHEN** an established TUI connection drops without an authentication rejection
- **THEN** the TUI reconnects with the existing capped backoff as before

##### Example: error-kind classification

| Error kind        | Auth rejection (terminal)? |
| ----------------- | -------------------------- |
| unauthenticated   | yes                        |
| unsupported       | no                         |
| invalid           | no                         |
| (any other)       | no                         |
