## MODIFIED Requirements

### Requirement: mDNS service discovery

The server SHALL announce `_fleety._tcp.local.` over mDNS, and the CLI and daemon SHALL browse for it as the last fallback when no URL is configured. `FLEETY_MDNS_DISABLED` SHALL, when set to any value, skip both announce and browse. When `FLEETY_ADDR` binds a wildcard address (`0.0.0.0`), the server SHALL auto-detect a single routable (non-loopback, non-wildcard) local IP to advertise — by opening a UDP socket and connecting it to a public address so the OS selects the outbound interface's IP, sending no packet — so discovery works out of the box on the exposed default. `FLEETY_MDNS_HOST_IP` SHALL, when set, force the advertised IP (overriding auto-detection, for multi-homed hosts). When neither an explicit host IP nor an auto-detected routable IP is available, the server SHALL skip the announcement (it never advertises a loopback or wildcard address). `FLEETY_MDNS_HOST` SHALL set the mDNS instance name (default the hostname).

#### Scenario: disabling mDNS skips announce and browse

- **WHEN** `FLEETY_MDNS_DISABLED` is set
- **THEN** the server does not announce and clients do not browse

#### Scenario: wildcard bind auto-detects a routable advertised IP

- **WHEN** `FLEETY_ADDR` binds `0.0.0.0`, `FLEETY_MDNS_HOST_IP` is unset, and the host has an outbound route
- **THEN** the server advertises the auto-detected routable IP rather than an unusable wildcard address

#### Scenario: an explicit host IP overrides auto-detection

- **WHEN** `FLEETY_ADDR` binds `0.0.0.0` and `FLEETY_MDNS_HOST_IP` is set
- **THEN** the server advertises that pinned IP instead of the auto-detected one
