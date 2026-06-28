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