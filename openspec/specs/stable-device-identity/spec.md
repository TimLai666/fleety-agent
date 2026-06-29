# stable-device-identity Specification

## Purpose

TBD - created by archiving change 'stable-device-identity'. Update Purpose after archive.

## Requirements

### Requirement: Device identity is machine-derived and stable

A device's id SHALL be derived from a stable OS machine identifier, so every
process on that machine (daemon, CLI, editor adapter) independently resolves the
**same** id, and two different machines never share one. An explicit override
SHALL remain available for environments where the machine id is shared or absent
(e.g. cloned VMs/containers). The hostname SHALL be a human-readable label, not
the identity.

#### Scenario: same-hostname machines no longer collide

- **WHEN** two machines that share a hostname connect
- **THEN** they have distinct device ids (their machine ids), so routing, storage, and ownership do not merge

#### Scenario: an override wins

- **WHEN** the device-id override is set
- **THEN** it is used as the device id instead of the machine id


<!-- @trace
source: stable-device-identity
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/device.rs
  - Cargo.toml
  - docs/env.md
  - crates/fleety-server/src/auth.rs
-->

---
### Requirement: Authenticated identity comes from the token

When a connection is authenticated, the server SHALL resolve the device id from
the authenticated token, not from the id the client asserts on the wire, so a
client cannot impersonate another device. Pairing SHALL bind the token to the
machine id. When authentication is disabled, the machine id reported on connect
SHALL be used directly (collision-free, though self-asserted).

#### Scenario: a spoofed wire id is ignored when authenticated

- **WHEN** an authenticated connection asserts a device id different from the one its token is bound to
- **THEN** the token's bound id is used, not the asserted one


<!-- @trace
source: stable-device-identity
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/device.rs
  - Cargo.toml
  - docs/env.md
  - crates/fleety-server/src/auth.rs
-->

---
### Requirement: Existing device data migrates losslessly, once

On connect, before the device id is used, the server SHALL perform a one-time,
verify-before-delete migration: if a legacy directory keyed by the reported
hostname exists and no directory for the machine id exists yet, the device's data
(conversations, audit history, device record) SHALL be moved to the machine-id
directory and any token rebound. The migration SHALL be idempotent and SHALL never
lose data on a crash (the source is removed only after the destination is written
and verified).

#### Scenario: a paired device keeps its data under the new id

- **WHEN** a previously paired device connects after the upgrade
- **THEN** its conversations, audit history, and device record are found under its machine id, and its token resolves to the machine id

#### Scenario: migration is idempotent

- **WHEN** a device that has already migrated connects again
- **THEN** no further move happens and its data is untouched

<!-- @trace
source: stable-device-identity
updated: 2026-06-29
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/device.rs
  - Cargo.toml
  - docs/env.md
  - crates/fleety-server/src/auth.rs
-->