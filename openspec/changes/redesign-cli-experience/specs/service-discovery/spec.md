## MODIFIED Requirements

### Requirement: mDNS is a sticky, fingerprint-guarded fallback in the resolver

Within the shared connection resolver, a saved current profile with an explicit URL SHALL connect automatically and SHALL rank above every discovery path. Authenticated endpoints previously learned from that same profile's `Welcome` SHALL be part of the saved profile rather than mDNS discovery. When no saved current URL or learned endpoint can be resolved, mDNS SHALL collect candidates for display and explicit selection only; the resolver SHALL NOT return an mDNS candidate as an operational target. Matching, mismatched, and absent TXT fingerprints SHALL NOT authorize sending a stored or caller-explicit token, sending a pairing code, persisting a `Welcome` token, accepting control frames, or assigning saved profile provenance. A user SHALL explicitly select an endpoint and complete pairing before that endpoint can become an operational profile. A credentialed profile without any saved endpoint SHALL require explicit endpoint selection and re-pair instead of falling through to mDNS.

#### Scenario: an enrolled device does not drift to mDNS

- **WHEN** a device has a current profile with a saved URL and an mDNS advertiser appears on the LAN
- **THEN** the resolver SHALL stay on the current profile's URL and SHALL NOT query mDNS

#### Scenario: learned endpoints remain profile-owned

- **GIVEN** profile `home` learned `ws://100.64.0.8:8787` from an authenticated `Welcome`
- **WHEN** its primary LAN endpoint is unreachable
- **THEN** the resolver SHALL try the learned endpoint as part of `home` without querying mDNS or treating another advertiser as trusted

##### Example: current profile wins over a live mDNS advertiser

- **GIVEN** `connections.toml` has `current = "home"` and `profiles.home.url = "ws://192.168.1.20:8787"`
- **AND** an mDNS advertiser is publishing `ws://192.168.1.99:8787` on the LAN
- **WHEN** the resolver runs with no command override and no `FLEETY_AGENT_URL`
- **THEN** it SHALL resolve `ws://192.168.1.20:8787` and SHALL NOT query mDNS

#### Scenario: mDNS-discovered server never receives a stored profile token

- **WHEN** automatic mDNS resolves a URL with a matching, mismatched, or absent TXT fingerprint
- **THEN** no saved profile token SHALL be attached, the candidate SHALL remain display／selection metadata only, and no operational result SHALL be returned

##### Example: copied matching fingerprint gets no token

- **GIVEN** saved profile `home` has token `home-token` and fingerprint `AA:BB`
- **AND** an mDNS advertiser publishes `ws://192.168.1.99:8787` with copied TXT fingerprint `AA:BB`
- **WHEN** automatic discovery evaluates the advertiser
- **THEN** it SHALL NOT attach `home-token`, attribute the target to `home`, or persist the discovered URL

#### Scenario: unconfigured discovery does not create an operational session

- **GIVEN** no saved current profile has an explicit URL
- **WHEN** mDNS discovers one or more Server advertisers
- **THEN** the candidates SHALL be displayed only through an explicit selection flow and none SHALL receive a token, pairing code, `Hello`, or control-session authority

#### Scenario: caller-explicit credentials do not follow automatic discovery

- **GIVEN** `FLEETY_TOKEN` or `FLEETY_PAIRING_CODE` is set without an explicit endpoint or saved current URL
- **WHEN** automatic resolution discovers an mDNS advertiser
- **THEN** resolution SHALL fail with explicit selection guidance before sending either credential

#### Scenario: a credentialed URL-less current profile requires repair

- **GIVEN** current profile `home` has a stored token but no URL
- **WHEN** the resolver runs without an explicit endpoint
- **THEN** it SHALL fail with explicit selection and re-pair guidance before querying mDNS
