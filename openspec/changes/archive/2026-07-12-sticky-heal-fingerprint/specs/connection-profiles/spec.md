## ADDED Requirements

### Requirement: Server fingerprints are pinned at pairing and on authenticated connections

When pairing succeeds (a `Welcome` that mints a token), the client SHALL pin the server's advertised identity fingerprint into the current profile alongside the token. On any later successfully authenticated connection whose `Welcome` carries a fingerprint: a profile without one SHALL be back-filled (trust-on-authenticated-connect, so devices enrolled before fingerprints exist need no re-pairing); a profile whose pinned fingerprint differs SHALL NOT be overwritten — the client SHALL warn that the server identity changed and point at re-pairing.

#### Scenario: pairing pins the fingerprint

- **WHEN** a device pairs with a server that advertises an identity fingerprint
- **THEN** the current profile stores that fingerprint next to the minted token

#### Scenario: an enrolled device back-fills its pin

- **WHEN** a device paired before fingerprints existed connects successfully and the Welcome carries a fingerprint
- **THEN** the profile gains that fingerprint without re-pairing

#### Scenario: an identity change warns and never overwrites

- **WHEN** an authenticated connection reports a fingerprint different from the pinned one
- **THEN** the client warns that the server identity changed, keeps the pinned value, and suggests re-pairing

### Requirement: Sticky connections heal by fingerprint when the address moves

When connecting to the current profile's URL fails, the profile carries a pinned fingerprint, and mDNS is not disabled, the client SHALL run one discovery scan and consider ONLY advertisers whose fingerprint exactly equals the pinned value: on a match at a different URL it SHALL persist the new URL to the profile, report that the server moved, and reconnect with the existing token; a match at the same URL, no match, or fingerprint-less advertisers SHALL leave the profile untouched and surface the original connection failure. The stored token SHALL never be sent to an advertiser whose fingerprint does not match the pin. A successful connection SHALL never trigger a scan (sticky behavior is unchanged on the happy path). The CLI SHALL heal at most once per command invocation; the daemon SHALL attempt the same heal inside its reconnect loop before each backoff sleep.

#### Scenario: the server moves to a new IP

- **WHEN** the saved URL stops answering and a scan finds an advertiser with the pinned fingerprint at a new URL
- **THEN** the profile's URL is updated, the client reports the move, and the connection proceeds with the existing token

#### Scenario: a different server on the LAN is never adopted

- **WHEN** the saved URL stops answering and the scan finds only advertisers with different or absent fingerprints
- **THEN** the profile is unchanged, no token is sent to any of them, and the original failure is reported

#### Scenario: healthy connections never scan

- **WHEN** the current profile's URL answers
- **THEN** no discovery scan runs and the connection proceeds as today
