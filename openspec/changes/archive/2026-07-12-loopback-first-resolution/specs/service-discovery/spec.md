## ADDED Requirements

### Requirement: The CLI prefers a co-located loopback server over mDNS

When the fleety CLI resolves a connection with no `--server`/`--url` override, no `FLEETY_AGENT_URL`, and no current profile, its discovery step SHALL first probe for a local server on loopback (`127.0.0.1:<port>`, the port taken from `FLEETY_ADDR` or the default) and, when one answers, resolve that loopback URL — ranking above mDNS. mDNS discovery SHALL be consulted only when no local loopback server answers. A loopback-resolved server SHALL carry no token, since a same-host connection is loopback-trusted by the server. This prevents a co-located CLI from resolving the host's own outward LAN IP via mDNS — a non-loopback address the server refuses without pairing. The preference applies to the CLI only; the daemon's resolution is unchanged.

#### Scenario: co-located CLI resolves loopback, not its own LAN IP

- **WHEN** the CLI resolves on the server host with no profile, the local server answers on `127.0.0.1`, and mDNS would advertise the host's own LAN IP
- **THEN** the resolver returns the `127.0.0.1` loopback URL (same-host trusted, no pairing), not the LAN IP

#### Scenario: no local server falls through to mDNS

- **WHEN** no server answers on loopback
- **THEN** the resolver proceeds to mDNS discovery exactly as before

##### Example: loopback wins over a live mDNS advertiser on the same host

- **GIVEN** a local server answers on `ws://127.0.0.1:8787`
- **AND** mDNS is advertising this host's own `ws://192.168.1.109:8787`
- **WHEN** the CLI resolves with no override, no `FLEETY_AGENT_URL`, and no current profile
- **THEN** it resolves `ws://127.0.0.1:8787` and reports it as this host's local server
