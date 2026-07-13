## ADDED Requirements

### Requirement: Daemon routing is not displaced by interactive sessions

Only a daemon-capable connection that advertises on-device tools SHALL occupy the routable device sender entry. An interactive CLI connection using the same stable device id SHALL receive its own replies but SHALL NOT replace or remove the daemon sender. Disconnect cleanup SHALL remove a sender only when the entry still belongs to the disconnecting connection.

#### Scenario: CLI connection does not replace daemon

- **GIVEN** fleetyd is connected under device id laptop
- **WHEN** an interactive fleety CLI connects under the same device id
- **THEN** device routing continues to send RunTool frames to fleetyd

#### Scenario: CLI disconnect does not remove daemon

- **GIVEN** fleetyd and an interactive CLI are connected under the same device id
- **WHEN** the CLI disconnects
- **THEN** the fleetyd sender remains routable

#### Scenario: stale daemon disconnect does not remove replacement

- **GIVEN** a newer fleetyd connection replaced an older daemon sender for the same device id
- **WHEN** the older connection finishes cleanup
- **THEN** the newer sender remains registered
