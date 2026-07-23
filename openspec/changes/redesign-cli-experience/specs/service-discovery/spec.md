## MODIFIED Requirements

### Requirement: mDNS is a sticky, fingerprint-guarded fallback in the resolver

Within the shared connection resolver, mDNS discovery SHALL rank below the current connection profile and SHALL be used only when no saved current URL can be resolved. Once a device is enrolled to a profile URL, the resolver SHALL stick to that URL and SHALL NOT drift to an mDNS-discovered Server. Every automatic mDNS result SHALL be treated as untrusted discovery metadata: matching, mismatched, and absent TXT fingerprints SHALL all receive no stored profile token and no persisted profile provenance. A credentialed URL-less current profile SHALL require explicit endpoint selection and re-pair instead of falling through to mDNS.

#### Scenario: an enrolled device does not drift to mDNS

- **WHEN** a device has a current profile with a saved URL and an mDNS advertiser appears on the LAN
- **THEN** the resolver SHALL stay on the current profile's URL and SHALL NOT query mDNS

##### Example: current profile wins over a live mDNS advertiser

- **GIVEN** `connections.toml` has `current = "home"` and `profiles.home.url = "ws://192.168.1.20:8787"`
- **AND** an mDNS advertiser is publishing `ws://192.168.1.99:8787` on the LAN
- **WHEN** the resolver runs with no command override and no `FLEETY_AGENT_URL`
- **THEN** it SHALL resolve `ws://192.168.1.20:8787` and SHALL NOT query mDNS

#### Scenario: mDNS-discovered server never receives a stored profile token

- **WHEN** automatic mDNS resolves a URL with a matching, mismatched, or absent TXT fingerprint
- **THEN** no saved profile token SHALL be attached and the result SHALL have mDNS provenance only

##### Example: copied matching fingerprint gets no token

- **GIVEN** saved profile `home` has token `home-token` and fingerprint `AA:BB`
- **AND** an mDNS advertiser publishes `ws://192.168.1.99:8787` with copied TXT fingerprint `AA:BB`
- **WHEN** automatic discovery evaluates the advertiser
- **THEN** it SHALL NOT attach `home-token`, attribute the target to `home`, or persist the discovered URL

#### Scenario: a credentialed URL-less current profile requires repair

- **GIVEN** current profile `home` has a stored token but no URL
- **WHEN** the resolver runs without an explicit endpoint
- **THEN** it SHALL fail with explicit selection and re-pair guidance before querying mDNS
