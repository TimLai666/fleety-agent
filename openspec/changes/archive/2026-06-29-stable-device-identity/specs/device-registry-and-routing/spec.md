## ADDED Requirements

### Requirement: Devices are registered under a stable machine-derived id

Device registration and routing SHALL key on a stable, machine-derived device id
(unique across machines, identical for every process on one machine), not a
client-asserted hostname. When a connection is authenticated, the id used for
registration and routing SHALL be the one bound to the authenticated token, so a
client cannot register or be routed to under another device's id. The hostname is
kept only as a display label on the device record.

#### Scenario: routing targets the right machine

- **WHEN** two same-hostname machines are connected
- **THEN** each has a distinct registered id, so a tool routed to one machine does not reach the other

#### Scenario: registration id is authenticated

- **WHEN** an authenticated device registers or is routed to
- **THEN** the id is taken from its token, not from a wire-asserted value
