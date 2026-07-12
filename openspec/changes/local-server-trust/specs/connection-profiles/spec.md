## ADDED Requirements

### Requirement: The local server is a first-class default switchable profile

When the CLI runs guided `fleety init` (no URL, on a TTY) it SHALL probe for a local server (connect to `ws://127.0.0.1:<port>`, port taken from `FLEETY_ADDR` or the default) with a short timeout before scanning the LAN. When a local server answers, it SHALL be presented at the top of the picker as the default choice, marked as local, and selecting it SHALL save a `local` profile pointing at it, make it current, and SKIP the pairing-code prompt (the connection is loopback-trusted). LAN-discovered servers SHALL still be listed below it. Switching to another server SHALL use the existing profile mechanism (`fleety server use <name>`), and switching back SHALL use the `local` profile; configuration and every other command land on whichever profile is current — unchanged. When no local server answers, guided init SHALL behave exactly as before (LAN scan, or usage guidance).

#### Scenario: local server is the default pick and needs no pairing

- **WHEN** guided `fleety init` runs on a host whose local server answers
- **THEN** `local` is listed first as the default, selecting it saves and uses the `local` profile, and no pairing code is requested

#### Scenario: local coexists with LAN servers and is switchable

- **WHEN** a local server and LAN servers are both present
- **THEN** the picker lists `local` first and the LAN servers below, and the user can pick either; a later `fleety server use` switches the current profile

#### Scenario: no local server falls back to the existing flow

- **WHEN** guided init runs on a host with no local server
- **THEN** the probe times out quietly and the LAN scan / usage guidance runs as before
