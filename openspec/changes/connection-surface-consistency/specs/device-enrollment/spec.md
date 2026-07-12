## ADDED Requirements

### Requirement: One-shot commands surface authentication rejections readably

The shared connect-and-hello helper used by the one-shot CLI commands (`pair-code`, `status`, `audit`, `rollback`, `conversations`, …) SHALL, when the server rejects the connection with an authentication error (error kind `unauthenticated`), return a concise human-readable message telling the user the device is not paired and how to fix it (`fleety pair <code>`, and that a code can be minted with `fleety pair-code` on the server host) — never the Debug representation of the internal protocol frame. Any other server `Error` SHALL surface the server's message readably, and any other unexpected frame SHALL yield a generic readable message rather than a `{variant:?}` dump. Successful handshakes and non-authentication failures SHALL be unchanged.

#### Scenario: an unpaired one-shot command is readable

- **WHEN** a one-shot command connects to an auth-required server without a valid token and is rejected as `unauthenticated`
- **THEN** it reports that the device is not paired and how to pair, without dumping the Debug form of the internal frame

#### Scenario: other errors still surface the server message

- **WHEN** the server rejects with a non-authentication `Error`
- **THEN** the command reports the server's message readably
