# runtime-configuration Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: Server bootstrap configuration

The server SHALL read `FLEETY_ADDR` for its WebSocket listen address (default `0.0.0.0:8787`), `FLEETY_AGENT_HOME` for its durable store root (default `$HOME/.fleety/agent`), `FLEETY_WORKSPACE` for the base directory that workspace tools resolve relative paths against (default the current working directory), and `FLEETY_SCHED_TICK` for the scheduler fire-loop interval in seconds (default `60`). Any unset variable SHALL use its default. The `FLEETY_ADDR` default exposes the server on all interfaces so it is reachable across devices out of the box; this is paired with authentication being required by default (see access policy), so an exposed address still needs a paired token to connect. An operator who wants loopback-only SHALL set `FLEETY_ADDR=127.0.0.1:8787` explicitly.

#### Scenario: defaults apply when unset

- **WHEN** the server starts with none of these variables set
- **THEN** it listens on `0.0.0.0:8787`, stores under `$HOME/.fleety/agent`, resolves relative paths against the current directory, and ticks the scheduler every 60 seconds

##### Example: bootstrap defaults

| Variable | Unset default |
| -------- | ------------- |
| `FLEETY_ADDR` | `0.0.0.0:8787` |
| `FLEETY_AGENT_HOME` | `$HOME/.fleety/agent` |
| `FLEETY_WORKSPACE` | current working directory |
| `FLEETY_SCHED_TICK` | `60` |


<!-- @trace
source: expose-server-by-default
updated: 2026-07-11
code:
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/mdns.rs
  - docs/env.md
  - docs/roadmap.md
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

---
### Requirement: Config write value validation

When a `config set` or interactive edit assigns a value to a known setting that carries a validator in the registry, the system SHALL validate the value before writing it to `config.toml` and SHALL reject any value the validator does not accept, leaving the stored configuration unchanged. This validation SHALL apply identically across every write surface: the shared `config set` dispatch used by `fleety`, `fleety-server`, and `fleetyd` (including remote `--target`), the non-TTY line-based editor, and the CLI ratatui edit screen. A setting that has no validator SHALL accept any value (pass-through), and an empty value SHALL continue to mean unset rather than being validated.

#### Scenario: invalid boolean is rejected

- **WHEN** a user runs `config set FLEETY_REQUIRE_AUTH abc` (the setting accepts only `0` or `1`)
- **THEN** the command SHALL fail without modifying `config.toml`

#### Scenario: invalid enum is rejected

- **WHEN** a user runs `config set FLEETY_FS_SCOPE ful` (the setting accepts only `full` or `workspace`)
- **THEN** the command SHALL fail without modifying `config.toml`

#### Scenario: out-of-domain numeric is rejected

- **WHEN** a user runs `config set FLEETY_CMD_TIMEOUT_SECS notanumber` (the setting accepts a non-negative integer)
- **THEN** the command SHALL fail without modifying `config.toml`

#### Scenario: valid value persists

- **WHEN** a user runs `config set FLEETY_POLICY require_approval` (an accepted enum member)
- **THEN** the value SHALL be written to `config.toml` under the setting's scope

#### Scenario: interactive edit rejects invalid value without saving

- **WHEN** the ratatui or line-based editor commits an invalid value for a validated setting
- **THEN** the editor SHALL NOT save the change and SHALL surface the validation error to the user

#### Scenario: unvalidated key passes through

- **WHEN** a user runs `config set FLEETY_TZ Anything/Here` (a setting with no validator)
- **THEN** the value SHALL be accepted and written unchanged


<!-- @trace
source: config-value-validation
updated: 2026-07-10
code:
  - README.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/storage.rs
  - Dockerfile
  - scripts/install.sh
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Validation error names accepted values

When a config write is rejected by a setting's validator, the returned error message SHALL name the key and describe the accepted values (the enum members, the boolean form `0|1`, the numeric domain, or the required URL scheme), so the user can correct the value without inspecting source code.

#### Scenario: enum error lists members

- **WHEN** `config set FLEETY_VOICE_AUDIO loud` is rejected (accepted: `auto`, `on`, `off`)
- **THEN** the error message SHALL name `FLEETY_VOICE_AUDIO` and list the accepted values `auto`, `on`, `off`

#### Scenario: URL error states required scheme

- **WHEN** `config set FLEETY_MODEL_BASE_URL notaurl` is rejected (requires an `http://` or `https://` URL)
- **THEN** the error message SHALL name the key and state that an `http`/`https` URL is required

<!-- @trace
source: config-value-validation
updated: 2026-07-10
code:
  - README.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/storage.rs
  - Dockerfile
  - scripts/install.sh
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-server/src/conn.rs
  - docs/env.md
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->