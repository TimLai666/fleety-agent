## ADDED Requirements

### Requirement: Loopback connections are trusted on the same host

A connection whose transport peer address is a loopback address (IPv4 `127.0.0.0/8` or IPv6 `::1`) SHALL be accepted without a token or pairing code even when authentication is required, because a same-host process can already read the server's token and config files — requiring a token adds friction, not security. The peer address SHALL be taken from the connection socket (via the server's connect-info), never from a request header or any client-supplied field that could be spoofed. `FLEETY_TRUST_LOOPBACK` SHALL gate this: any value other than `0` (including unset) trusts loopback; `0` requires authentication even on loopback (for multi-tenant hosts or a reverse proxy that forwards remote connections over loopback). A trusted-loopback acceptance SHALL be reflected back to the client in `Welcome` so it can skip pairing prompts. Non-loopback (LAN/remote) connections SHALL be authenticated exactly as before.

#### Scenario: same-host client connects without pairing

- **WHEN** a client connects from a loopback peer to an auth-required server with loopback trust enabled and presents no token
- **THEN** the server accepts the connection and marks it loopback-trusted in `Welcome`

#### Scenario: loopback trust can be disabled

- **WHEN** `FLEETY_TRUST_LOOPBACK=0` and a loopback client presents no token
- **THEN** the server rejects it exactly like any unauthenticated connection

#### Scenario: remote connections are unaffected

- **WHEN** a client connects from a non-loopback (LAN) address without a token
- **THEN** the server requires authentication as before, regardless of the loopback-trust setting

##### Example: peer-address classification

| Peer address     | Loopback? |
| ---------------- | --------- |
| 127.0.0.1        | yes       |
| 127.0.5.9        | yes       |
| ::1              | yes       |
| 192.168.1.10     | no        |
| 10.0.0.4         | no        |
