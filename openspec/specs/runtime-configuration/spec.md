# runtime-configuration Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: Server bootstrap configuration

The server SHALL read `FLEETY_ADDR` for its WebSocket listen address (default `127.0.0.1:8787`), `FLEETY_AGENT_HOME` for its durable store root (default `$HOME/.fleety/agent`), `FLEETY_WORKSPACE` for the base directory that workspace tools resolve relative paths against (default the current working directory), and `FLEETY_SCHED_TICK` for the scheduler fire-loop interval in seconds (default `60`). Any unset variable SHALL use its default.

#### Scenario: defaults apply when unset

- **WHEN** the server starts with none of these variables set
- **THEN** it listens on `127.0.0.1:8787`, stores under `$HOME/.fleety/agent`, resolves relative paths against the current directory, and ticks the scheduler every 60 seconds

##### Example: bootstrap defaults

| Variable | Unset default |
| -------- | ------------- |
| `FLEETY_ADDR` | `127.0.0.1:8787` |
| `FLEETY_AGENT_HOME` | `$HOME/.fleety/agent` |
| `FLEETY_WORKSPACE` | current working directory |
| `FLEETY_SCHED_TICK` | `60` |


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
### Requirement: Access policy and authentication

The server SHALL read `FLEETY_POLICY` (default `full_access`); when set to `require_approval` it SHALL gate every non-read tool through the approval flow. It SHALL read `FLEETY_REQUIRE_AUTH` (default `0`); when set to `1` it SHALL require a valid token or pairing code on every `Hello`. `FLEETY_TOKEN` SHALL provide a bootstrap admin token usable to pair the first device.

#### Scenario: approval gating toggles with policy

- **WHEN** `FLEETY_POLICY=require_approval` and a mutating tool is invoked
- **THEN** the call is routed through the approval flow before executing
- **WHEN** `FLEETY_POLICY` is unset
- **THEN** the policy is `full_access` and mutating tools run without per-call approval

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