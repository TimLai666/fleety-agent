## ADDED Requirements

### Requirement: Pairing failures surface readable errors

When `fleety pair` receives a reply that is not a successful `Welcome`, the CLI SHALL report a concise, human-readable message describing the failure and the next step, and SHALL NOT print the Debug representation of internal protocol types to the user. A server `Error` reply SHALL surface the server's message; a `Welcome` with no token SHALL explain that pairing requires the server to run in auth-required mode; any other unexpected frame SHALL yield a generic readable message rather than a `{variant:?}` dump.

#### Scenario: unexpected reply is readable

- **WHEN** the server answers a pairing Hello with a frame that is neither a `Welcome` nor an `Error`
- **THEN** the CLI prints a concise, human-readable failure message and exits non-zero, without dumping the Debug form of the internal message type

#### Scenario: server error is surfaced verbatim

- **WHEN** the server answers pairing with an `Error` frame
- **THEN** the CLI reports the server's error message in a readable form, not a Debug dump
