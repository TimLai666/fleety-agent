# runtime-configuration Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: Server bootstrap configuration

The server SHALL read `FLEETY_ADDR` for its WebSocket listen address (default `0.0.0.0:8787`), `FLEETY_AGENT_HOME` for its durable store root (default `$HOME/.fleety/agent`), `FLEETY_WORKSPACE` for the base directory that workspace tools resolve relative paths against (default the current working directory), and `FLEETY_SCHED_TICK` for the scheduler fire-loop interval in seconds (default `60`). Any unset variable SHALL use its default. The `FLEETY_ADDR` default exposes the server on all interfaces so it is reachable across devices out of the box; this is paired with authentication being required by default (see access policy), so an exposed address still needs a paired token to connect. An operator who wants loopback-only SHALL set `FLEETY_ADDR=127.0.0.1:8787` explicitly.

When the configured listen address is the IPv4 wildcard (`0.0.0.0`) or the IPv4 loopback (`127.0.0.1`), the server SHALL additionally attempt to bind a best-effort IPv6 companion listener on the same port (`[::]` and `[::1]` respectively), because on a dual-stack host a client that spells the endpoint `localhost` resolves to `::1` first and pays a multi-second fallback when nothing listens there. A companion bind failure SHALL NOT fail startup: the server SHALL continue IPv4-only and log that the companion was not established. Any other explicitly configured address SHALL be bound exactly as given, with no companion.

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

#### Scenario: both address families reach a default server

- **WHEN** the server starts with `FLEETY_ADDR` unset (or set to a `0.0.0.0` or `127.0.0.1` address) on a host where IPv6 is available
- **THEN** connections to both the IPv4 address and its IPv6 companion (`::` / `::1`) on the same port are accepted immediately

#### Scenario: a failed companion bind degrades to IPv4-only

- **WHEN** the IPv6 companion cannot be bound (no IPv6, or the port is taken on IPv6)
- **THEN** the server starts and serves IPv4 exactly as before, and logs that the companion listener was not established

#### Scenario: an explicit address is bound exactly

- **WHEN** `FLEETY_ADDR` names any address other than a `0.0.0.0` or `127.0.0.1` form
- **THEN** the server binds exactly that address and starts no companion listener


<!-- @trace
source: localhost-dual-stack-reachability
updated: 2026-08-02
code:
  - docs/env.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/transport.rs
  - AGENTS.md
  - crates/fleety-cli/src/main.rs
-->

---
### Requirement: Access policy and authentication

The server SHALL read `FLEETY_POLICY` (default `full_access`). The accepted policies SHALL be `full_access`, `require_approval`, and `auto_review`. Under `require_approval`, every non-read tool SHALL use the interactive approval flow. Under `auto_review`, read tools SHALL run directly and every mutate or critical tool SHALL use the unattended cheap-model review defined by the `auto-review` capability. The server SHALL read `FLEETY_REQUIRE_AUTH` (default `1`); any value other than an explicit `0` SHALL enable connection authentication, subject to the loopback trust behavior defined by `authentication-default-on`. `FLEETY_TOKEN` SHALL provide a bootstrap admin token usable to pair the first device. The server SHALL read `FLEETY_AUTO_REVIEW_TIMEOUT_SECS` with a positive default and SHALL use it as the maximum review wait; invalid or non-positive values SHALL use the documented default.

#### Scenario: full access remains the default

- **WHEN** `FLEETY_POLICY` is unset
- **THEN** the policy is `full_access` and mutate tools run without per-call approval

#### Scenario: interactive approval remains available

- **WHEN** `FLEETY_POLICY=require_approval` and a mutate or critical tool is invoked
- **THEN** the call is routed through the interactive approval flow before executing

#### Scenario: auto review is selectable

- **WHEN** `FLEETY_POLICY=auto_review` and a mutate or critical tool is invoked
- **THEN** the call is routed through the cheap-model review and no human approval request is emitted

#### Scenario: invalid auto-review timeout uses the safe default

- **WHEN** `FLEETY_AUTO_REVIEW_TIMEOUT_SECS` is unset, non-numeric, or non-positive
- **THEN** the server uses the documented positive default and never waits without a bound


<!-- @trace
source: auto-review
updated: 2026-08-15
code:
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/agent-core/src/agent.rs
  - docs/tools.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/event.rs
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/scheduler.rs
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
- **WHEN** a user runs `config set FLEETY_POLICY auto_review` (an accepted enum member)
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