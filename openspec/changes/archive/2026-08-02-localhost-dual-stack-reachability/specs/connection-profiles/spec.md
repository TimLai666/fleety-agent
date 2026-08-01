## ADDED Requirements

### Requirement: Dialing a localhost endpoint prefers IPv4

When a client dials an endpoint whose host is exactly `localhost`, the transport SHALL connect to `127.0.0.1` on the same port instead of resolving the name, because on a dual-stack host `localhost` resolves to `::1` first and a server bound only to IPv4 costs a multi-second fallback — long enough to exceed every per-candidate budget in the connection sweep, making such an endpoint not merely slow but unreachable as a candidate.

The rewrite SHALL apply at the single transport dial choke point shared by every client path (the CLI one-shot connect and the daemon reconnect loop), so no per-surface handling is needed. It SHALL affect only where the socket connects: the URL as displayed, stored in a profile, and used for identity SHALL remain the user's spelling. A host that is anything other than the exact name `localhost` — an IP literal (including `[::1]`), any other hostname — SHALL be dialed exactly as spelled. Reaching a server bound only to IPv6 loopback therefore requires spelling the endpoint `[::1]`, which SHALL be documented.

#### Scenario: localhost dials the IPv4 loopback immediately

- **WHEN** a client dials `ws://localhost:8787` and a server listens only on `127.0.0.1:8787`
- **THEN** the connection is established without a name-resolution fallback delay, and the profile keeps the URL spelled `localhost`

#### Scenario: every other host is dialed as spelled

- **WHEN** a client dials an endpoint whose host is `[::1]`, an IPv4 literal, or any hostname other than `localhost`
- **THEN** the transport connects to exactly that host with no rewrite
