# service-discovery Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: mDNS service discovery

The server SHALL announce `_fleety._tcp.local.` over mDNS, and the CLI and daemon SHALL browse for it as the last fallback when no URL is configured. `FLEETY_MDNS_DISABLED` SHALL, when set to any value, skip both announce and browse. `FLEETY_MDNS_HOST_IP` SHALL force the advertised IP and SHALL be required when `FLEETY_ADDR` binds to `0.0.0.0` (the server does not enumerate interfaces). `FLEETY_MDNS_HOST` SHALL set the mDNS instance name (default the hostname).

#### Scenario: disabling mDNS skips announce and browse

- **WHEN** `FLEETY_MDNS_DISABLED` is set
- **THEN** the server does not announce and clients do not browse

#### Scenario: wildcard bind needs an explicit advertised IP

- **WHEN** `FLEETY_ADDR` binds `0.0.0.0` and `FLEETY_MDNS_HOST_IP` is set
- **THEN** the server advertises that IP rather than an unusable wildcard address

<!-- @trace
source: baseline-config-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-discuss.md
  - .spectra.yaml
  - CLAUDE.md
  - .opencode/commands/spectra-ask.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-debug/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
-->

---
### Requirement: mDNS is a sticky, fingerprint-guarded fallback in the resolver

Within the shared connection resolver, mDNS discovery SHALL rank below the current connection profile — it is used only when there is no current profile to resolve. Once a device is enrolled to a profile, the resolver SHALL stick to that profile's URL and SHALL NOT drift to an mDNS-discovered server. When mDNS is used, the resolver SHALL NOT send a profile's existing token to a discovered URL whose server fingerprint does not match that profile's recorded fingerprint, so a rogue mDNS advertiser cannot harvest an enrolled device's token.

#### Scenario: an enrolled device does not drift to mDNS

- **WHEN** a device has a current profile and an mDNS advertiser appears on the LAN
- **THEN** the resolver stays on the current profile's URL and ignores the mDNS result

##### Example: current profile wins over a live mDNS advertiser

- **GIVEN** connections.toml has `current = "home"` and `profiles.home.url = "ws://192.168.1.20:8787"`
- **AND** an mDNS advertiser is publishing `ws://192.168.1.99:8787` on the LAN
- **WHEN** the resolver runs with no `--server`/`--url` override and no `FLEETY_AGENT_URL`
- **THEN** it resolves `ws://192.168.1.20:8787` and never queries mDNS

#### Scenario: mDNS-discovered server does not receive a mismatched profile's token

- **WHEN** mDNS resolves a URL whose fingerprint does not match a profile's recorded fingerprint
- **THEN** that profile's token is not sent to the discovered URL

##### Example: rogue advertiser with a wrong fingerprint gets no token

- **GIVEN** there is no current profile but `profiles.home` recorded `fingerprint = "AA:BB"` and a token
- **AND** mDNS resolves `ws://192.168.1.99:8787` whose server presents fingerprint `"CC:DD"`
- **WHEN** the resolver falls through to mDNS and evaluates the discovered URL
- **THEN** it returns the URL with **no** token attached, so `"CC:DD"` never receives `home`'s token

<!-- @trace
source: connection-profiles
updated: 2026-07-10
code:
  - crates/fleety-cli/src/server.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->