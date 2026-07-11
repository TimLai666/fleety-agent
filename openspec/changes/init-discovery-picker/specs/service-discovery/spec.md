## ADDED Requirements

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
