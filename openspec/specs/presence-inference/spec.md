# presence-inference Specification

## Purpose

TBD - created by archiving change 'presence-inference'. Update Purpose after archive.

## Requirements

### Requirement: Devices self-report co-location signals under opt-in

A daemon SHALL compute a network fingerprint of its current LAN (default-gateway MAC plus subnet, optionally the set of mDNS-discovered Fleety peers) and report it periodically over the existing authenticated connection, ONLY when presence tracking is enabled for that device. When presence is disabled (the default), the daemon SHALL NOT compute or send any co-location signal. The fingerprint SHALL be stored hashed, never as a raw MAC address. When the fingerprint cannot be determined, the daemon SHALL report an absent fingerprint rather than fail or crash.

#### Scenario: presence disabled sends nothing

- **WHEN** a daemon runs with presence tracking disabled (the default)
- **THEN** it computes and sends no co-location report, and the server records no presence data for it

#### Scenario: opted-in device reports its LAN fingerprint

- **WHEN** a daemon has presence tracking enabled and is on a LAN whose fingerprint can be determined
- **THEN** it periodically sends a co-location report carrying the hashed fingerprint and subnet over the existing connection

#### Scenario: fingerprint cannot be determined

- **WHEN** presence is enabled but the LAN fingerprint cannot be read from the OS
- **THEN** the daemon reports an absent fingerprint and does not crash, and the server leaves the device's site unchanged


<!-- @trace
source: presence-inference
updated: 2026-07-03
code:
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - prompts/memory.md
  - crates/fleety-server/src/gc.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->

---
### Requirement: The server maps fingerprints to sites and updates current site

The server SHALL maintain, per site, a set of known network fingerprints. On receiving a co-location report from an opted-in device, the server SHALL update that device's current site to the site whose fingerprint set contains the reported fingerprint. If the reported fingerprint matches no site, the server SHALL leave the site as `unknown` and SHALL surface that the fingerprint is unbound. The server SHALL bind a fingerprint to a site only through an explicit action, never by inferring an arbitrary site from an unknown fingerprint. A co-location report from a device that has not opted in SHALL be ignored and SHALL NOT be recorded.

#### Scenario: known fingerprint updates the site

- **WHEN** an opted-in device reports a fingerprint that is bound to site `home`
- **THEN** the server sets that device's current site to `home`

#### Scenario: unknown fingerprint stays unknown

- **WHEN** an opted-in device reports a fingerprint bound to no site
- **THEN** the server leaves the device's current site as `unknown` and marks the fingerprint as unbound so it can be bound explicitly

#### Scenario: binding a fingerprint to a site

- **WHEN** an operator binds a device's currently reported fingerprint to site `home`
- **THEN** that fingerprint is added to `home`'s fingerprint set, and subsequent reports of it set the device's site to `home`


<!-- @trace
source: presence-inference
updated: 2026-07-03
code:
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - prompts/memory.md
  - crates/fleety-server/src/gc.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->

---
### Requirement: Devices carry a home-site baseline distinct from current site

A device record SHALL carry a `home_site` field, separate from the current `site`, representing where the device usually is. `home_site` SHALL be set explicitly and default to empty. The system SHALL NOT auto-derive `home_site` in this capability.

#### Scenario: setting a home site

- **WHEN** an operator sets a device's `home_site` to a registered site
- **THEN** the device record persists `home_site` independently of its changing current `site`

#### Scenario: home site defaults empty

- **WHEN** a device has never had a `home_site` set
- **THEN** its `home_site` reads as empty and presence answers treat the baseline as unknown


<!-- @trace
source: presence-inference
updated: 2026-07-03
code:
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - prompts/memory.md
  - crates/fleety-server/src/gc.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->

---
### Requirement: The server records a presence timeline of site changes

The server SHALL append a presence-timeline event whenever an opted-in device's current site actually changes. Each event SHALL carry a timestamp, the device, the previous site, the new site, the signal source, and a confidence value. A report that does not change the current site SHALL NOT append a duplicate event. The timeline SHALL be persisted and SHALL be subject to the existing retention policy.

#### Scenario: a site change is recorded once

- **WHEN** an opted-in device's current site changes from `away` to `home`
- **THEN** the server appends one timeline event recording the transition with a timestamp and confidence

#### Scenario: an unchanged site is not re-recorded

- **WHEN** an opted-in device reports the same site it is already at
- **THEN** no new timeline event is appended


<!-- @trace
source: presence-inference
updated: 2026-07-03
code:
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - prompts/memory.md
  - crates/fleety-server/src/gc.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->

---
### Requirement: Presence is answered probabilistically with confidence and caveats

The agent SHALL be able to query presence at the site level (which devices are at a site and whether a person is likely present) and at the device level (a device's current site, home site, and how long it has been there). Every presence answer SHALL carry a confidence value and the reasons behind it, and SHALL state that reachability does not imply presence. A presence answer SHALL NOT be expressed as a certainty. The confidence computation SHALL be a pure function of the device set, their mobility, and their home sites.

#### Scenario: site-level presence with confidence

- **WHEN** the agent asks whether anyone is present at site `home`
- **THEN** it receives the devices currently at `home`, a probabilistic person-present confidence with reasons, and a caveat that reachability is not presence

#### Scenario: a mobile device away from its home site signals departure

- **WHEN** a mobile device whose `home_site` is `home` has a current site other than `home`
- **THEN** the device-level presence answer reflects a likely departure with a confidence value, not a certain claim

##### Example: person-present signal strength

| Devices at `home`                                  | person_present confidence | Reason |
| -------------------------------------------------- | ------------------------- | ------ |
| only a stationary desktop present                  | low                       | stationary device shows the site is reachable, not that a person is there |
| a mobile phone whose home_site is `home` present   | higher                    | a personal mobile device at its home site is a stronger presence signal |
| a mobile phone whose home_site is `home` is `away` | departure-leaning         | the usually-home mobile device is elsewhere |


<!-- @trace
source: presence-inference
updated: 2026-07-03
code:
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - prompts/memory.md
  - crates/fleety-server/src/gc.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->

---
### Requirement: Presence tracking is per-device opt-in and off by default

Presence tracking SHALL be gated by two independent opt-ins, both defaulting to off: a daemon-side switch that controls whether co-location signals are computed and sent, and a server-side per-device flag that controls whether the device's site changes and timeline are recorded. When either gate is off for a device, the server SHALL NOT record presence data for it. Presence data SHALL belong to the user and SHALL be removable.

#### Scenario: server-side opt-in off blocks recording

- **WHEN** a device's server-side presence opt-in is off but it somehow sends a co-location report
- **THEN** the server records no site change and no timeline event for it

#### Scenario: both gates on enables tracking

- **WHEN** both the daemon switch and the server-side per-device flag are on
- **THEN** the device's co-location reports update its site and timeline as specified

<!-- @trace
source: presence-inference
updated: 2026-07-03
code:
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - prompts/memory.md
  - crates/fleety-server/src/gc.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
-->