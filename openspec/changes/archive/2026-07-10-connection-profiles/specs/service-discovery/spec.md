## ADDED Requirements

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
