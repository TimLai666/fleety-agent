## ADDED Requirements

### Requirement: mDNS service discovery

The server SHALL announce `_fleety._tcp.local.` over mDNS, and the CLI and daemon SHALL browse for it as the last fallback when no URL is configured. `FLEETY_MDNS_DISABLED` SHALL, when set to any value, skip both announce and browse. `FLEETY_MDNS_HOST_IP` SHALL force the advertised IP and SHALL be required when `FLEETY_ADDR` binds to `0.0.0.0` (the server does not enumerate interfaces). `FLEETY_MDNS_HOST` SHALL set the mDNS instance name (default the hostname).

#### Scenario: disabling mDNS skips announce and browse

- **WHEN** `FLEETY_MDNS_DISABLED` is set
- **THEN** the server does not announce and clients do not browse

#### Scenario: wildcard bind needs an explicit advertised IP

- **WHEN** `FLEETY_ADDR` binds `0.0.0.0` and `FLEETY_MDNS_HOST_IP` is set
- **THEN** the server advertises that IP rather than an unusable wildcard address
