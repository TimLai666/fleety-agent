# service-discovery Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

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


<!-- @trace
source: expose-server-by-default
updated: 2026-07-11
code:
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/mdns.rs
  - docs/env.md
  - docs/roadmap.md
-->

---
### Requirement: mDNS is a sticky, fingerprint-guarded fallback in the resolver

Within the shared connection resolver, mDNS discovery SHALL rank below the current connection profile — it is used only when there is no current profile to resolve. Once a device is enrolled to a profile, the resolver SHALL stick to that profile's URL and SHALL NOT drift to an mDNS-discovered server. When mDNS is used, the resolver SHALL NOT send a profile's existing token to a discovered URL whose server fingerprint does not match that profile's recorded fingerprint, so a rogue mDNS advertiser cannot harvest an enrolled device's token.

#### Scenario: an enrolled device does not drift to mDNS

- **WHEN** a device has a current profile and an mDNS advertiser appears on the LAN
- **THEN** the resolver stays on the current profile's URL and ignores the mDNS result

##### Example: current profile wins over a live mDNS advertiser

- **GIVEN** connections.toml has `current = "home"` and `profiles.home.url = "ws://192.168.1.20:8787"`
- **AND** an mDNS advertiser is publishing `ws://192.168.1.99:8787` on the LAN
- **WHEN** the resolver runs with no `--server`/`--url` override and no `FLEETY_AGENT_URL`
- **THEN** it resolves `ws://192.168.1.20:8787` and never queries mDNS

#### Scenario: mDNS-discovered server does not receive a mismatched profile's token

- **WHEN** mDNS resolves a URL whose fingerprint does not match a profile's recorded fingerprint
- **THEN** that profile's token is not sent to the discovered URL

##### Example: rogue advertiser with a wrong fingerprint gets no token

- **GIVEN** there is no current profile but `profiles.home` recorded `fingerprint = "AA:BB"` and a token
- **AND** mDNS resolves `ws://192.168.1.99:8787` whose server presents fingerprint `"CC:DD"`
- **WHEN** the resolver falls through to mDNS and evaluates the discovered URL
- **THEN** it returns the URL with **no** token attached, so `"CC:DD"` never receives `home`'s token

<!-- @trace
source: connection-profiles
updated: 2026-07-10
code:
  - crates/fleety-cli/src/server.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Interactive discovery lists every advertised server

For guided onboarding, the CLI SHALL provide a collecting discovery mode that browses `_fleety._tcp.local.` for a fixed window and returns **every** server resolved in it — each entry carrying a display name derived from the advertised instance name (the `fleety-` prefix stripped; the URL stands in when no name is available) and the `ws://ip:port` URL — de-duplicated by URL, in discovery order. The existing implicit single-result fallback used by the connection resolver SHALL keep its early-return behavior unchanged. When mDNS is disabled or the browse cannot start, collecting discovery SHALL return an empty list rather than failing.

#### Scenario: multiple servers are all collected

- **WHEN** two servers advertise on the LAN during the collection window
- **THEN** the collecting discovery returns both entries with their display names and URLs

#### Scenario: duplicate announcements collapse

- **WHEN** the same server is resolved more than once during the window
- **THEN** the returned list contains it once

##### Example: name derivation

| Advertised instance | Display name |
| ------------------- | ------------ |
| fleety-mini         | mini         |
| fleety-nas-01       | nas-01       |
| (none resolved)     | the ws URL   |

#### Scenario: disabled discovery yields an empty list

- **WHEN** `FLEETY_MDNS_DISABLED` is set and collecting discovery runs
- **THEN** it returns an empty list without browsing

<!-- @trace
source: init-discovery-picker
updated: 2026-07-11
code:
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/main.rs
-->