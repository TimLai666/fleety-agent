## ADDED Requirements

### Requirement: A pairing code can be minted over the connection

The client SHALL be able to request the connected server to mint a short-lived pairing code, via a `MintPairingCode` request and a reply carrying either the code or an actionable error. The server SHALL mint (through the same store the first-run code and `pair_create` use) only when authentication is required; when authentication is disabled it SHALL reply with an error explaining that pairing codes are not used and how to enable auth. Because a connection only reaches this point after passing Hello (a valid token or same-host loopback trust), no additional privilege check is needed — an unauthenticated LAN peer is already rejected before it can request a code. The CLI SHALL expose this as `fleety pair-code`, printing the minted code and how to redeem it on another device; against a server too old to support the request it SHALL print a version hint.

#### Scenario: minting on an auth-required server

- **WHEN** `fleety pair-code` runs against an auth-required server it can reach (loopback-trusted or token-authenticated)
- **THEN** the server mints a short-lived code and the CLI prints it with `fleety pair <code>` guidance

#### Scenario: minting is refused when auth is disabled

- **WHEN** `fleety pair-code` runs against a server with authentication disabled
- **THEN** the reply carries an error explaining pairing codes are unused and how to enable auth, and no code is printed

#### Scenario: an old server yields a version hint

- **WHEN** the connected server does not support the mint request
- **THEN** the CLI reports that the server is too old and suggests updating it
