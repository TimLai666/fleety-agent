## ADDED Requirements

### Requirement: The acting user is a hard privacy boundary

Every read of conversations, recall, and per-user memory SHALL be scoped to the acting user through a data-layer guard: a turn SHALL only read the acting user's data, plus anything explicitly granted to it. Cross-user access SHALL be default-deny. Enforcement SHALL be at the data layer, not only in the prompt. Guest SHALL have no access to any real user's private data.

#### Scenario: a turn reads only the acting user's data

- **WHEN** the acting user is A and the agent reads conversations or memory
- **THEN** it sees A's data and never B's

#### Scenario: cross-user is denied by default

- **WHEN** a turn attempts to read another user's data with no grant
- **THEN** the access is denied

#### Scenario: Guest reads no private data

- **WHEN** the acting user is Guest
- **THEN** no real user's conversations, recall, or memory are readable

### Requirement: No disclosure of another user's content, timing, or existence

The agent SHALL NOT disclose another user's information without that user's authorization — not the content, not when they used the system, and not whether they exist or whether a topic was discussed with them. A denied cross-user access SHALL return a uniform response that does not distinguish "no such data" from "exists but forbidden", so the refusal itself reveals nothing.

#### Scenario: existence is not leaked by a refusal

- **WHEN** the agent is asked about another user's data it may not access
- **THEN** it responds in a uniform "not available to you" way that does not reveal whether that user or that data exists

#### Scenario: timing is not leaked

- **WHEN** the agent is asked when another user last used the system, without authorization
- **THEN** it does not reveal that timing

### Requirement: Cross-user access requires an explicit grant

Access to another user's data SHALL require an explicit grant from that user, covering a defined scope; absent a matching grant, access SHALL be denied. Grants SHALL be consulted by the data-layer guard. A corrupt or unreadable grant store SHALL fail closed (deny).

#### Scenario: granted access within scope is allowed

- **WHEN** user A has granted the acting principal access to a scope, and the requested data is within it
- **THEN** the access is allowed

#### Scenario: access outside the grant is denied

- **WHEN** the requested data is outside any grant the acting principal holds
- **THEN** the access is denied

### Requirement: Conversations are stored per user, with device recorded, migrated losslessly

Conversations SHALL be stored under the owning user (`users/<user>/conversations/`), with each event recording the device it occurred on. Existing per-device conversations SHALL be migrated once under their device's owner (or a reserved unattributed bucket when the device has no owner), the migration SHALL be lossless and idempotent, and resume/lookup by conversation id SHALL continue to work via an id-to-owner index.

#### Scenario: existing conversations migrate under their owner

- **WHEN** the system first runs after this change and a device has an owner
- **THEN** that device's conversations are placed under the owner with the device recorded per event, and nothing is lost

#### Scenario: resume still works after migration

- **WHEN** a client resumes a conversation by id after migration
- **THEN** the id resolves to the right user's conversation and replays correctly

#### Scenario: a device with no owner

- **WHEN** a device without an owner is migrated
- **THEN** its conversations go to a reserved unattributed bucket rather than being lost or attributed to a real user
