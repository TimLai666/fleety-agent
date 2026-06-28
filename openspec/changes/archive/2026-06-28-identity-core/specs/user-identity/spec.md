## ADDED Requirements

### Requirement: Each turn resolves an acting user

The server SHALL resolve an acting user for every turn, layered on the existing per-device token: an explicitly asserted user (when valid and authorized for the device) is used; otherwise a personal device's owner is used; otherwise the acting user is Guest (an identified-as-unknown principal). The per-device token continues to authenticate the device/transport; the acting user is attached on top. Resolution SHALL be total — every turn yields a User or Guest, never an undefined state.

#### Scenario: personal device resolves to its owner

- **WHEN** a turn arrives on a device that has an owner and no asserted user
- **THEN** the acting user is that owner

#### Scenario: asserted user is honored when authorized

- **WHEN** a turn carries a valid asserted user that the device permits
- **THEN** the acting user is the asserted user

#### Scenario: unidentified falls to Guest

- **WHEN** a turn has no asserted user and the device has no owner
- **THEN** the acting user is Guest

##### Example: acting-user resolution

| Device owner | Asserted user | Device shares to | Resolved acting user |
| ------------ | ------------- | ---------------- | -------------------- |
| alice | (none) | — | alice |
| (none) | bob | bob | bob |
| (none) | bob | (not listed) | Guest (fail closed) |
| (none) | (none) | — | Guest |

### Requirement: Devices record ownership

A device record SHALL carry ownership: an optional `owner` user, a list of authorized `users`, and a `shared` flag, so a device can be personal, multi-user, or public. These fields SHALL be additive — a device record without them loads with safe defaults (no owner, no authorized users, not shared).

#### Scenario: ownership is recorded and read back

- **WHEN** a device is given an owner / authorized users / shared flag
- **THEN** those are persisted on the device record and used to resolve the acting user

#### Scenario: legacy device record still loads

- **WHEN** an existing device record without ownership fields is read
- **THEN** it loads with no owner, no authorized users, and not shared, and the system keeps working

### Requirement: The agent's user profile is the acting user's

The core-memory USER block injected each turn SHALL be the acting user's profile, stored per user; for Guest it SHALL be a neutral placeholder containing no personal data. Agent-global memory (the ME and TODO blocks) SHALL remain global and unchanged. There SHALL no longer be a single shared user profile.

#### Scenario: each user sees their own profile

- **WHEN** the acting user is a particular person
- **THEN** the USER block injected is that person's profile, not a shared one

#### Scenario: Guest gets a neutral profile

- **WHEN** the acting user is Guest
- **THEN** the USER block is a neutral placeholder with no personal data, while ME and TODO remain the agent-global blocks

### Requirement: The acting-user assertion is additive and backward compatible

The wire field that carries an asserted user SHALL be optional and additive: clients that do not send it continue to work (resolution falls back to the device owner or Guest), and the protocol version SHALL NOT change. An assertion SHALL only identify the acting user; it SHALL NOT by itself grant access to another user's data.

#### Scenario: older client without the field

- **WHEN** a client that does not send the assertion field connects
- **THEN** turns resolve via device owner or Guest, and the protocol version is unchanged

#### Scenario: assertion identifies but does not authorize cross-user access

- **WHEN** a turn asserts a user
- **THEN** it sets who the acting user is, but does not in itself unlock another user's data
