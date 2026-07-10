# two-tier-provider-model Specification

## Purpose

TBD - created by archiving change 'provider-model-two-tier'. Update Purpose after archive.

## Requirements

### Requirement: Providers are a type-tagged enum separating endpoint from model

A provider in `providers.toml` SHALL carry a `type` that selects its authentication and endpoint shape. `type = "api"` SHALL require a `base_url`, SHALL treat `key` as optional, and SHALL NOT carry an oauth token. `type = "oauth:codex"` SHALL source its bearer token from the per-provider Codex OAuth login and SHALL NOT carry a `base_url` or `key`. The provider owns endpoint/secret/auth; the model name is not a provider field. The set of `type` values SHALL be an extensible registry — adding a new auth type does not require editing a core conditional — and an unknown `type` SHALL be rejected at parse with an error that lists the known types.

#### Scenario: an api provider requires a base_url and forbids a token

- **WHEN** a `type = "api"` provider is written with no `base_url`, or with an oauth token field
- **THEN** validation rejects it before the file is written

#### Scenario: an oauth provider forbids base_url and key

- **WHEN** a `type = "oauth:codex"` provider is written with a `base_url` or `key`
- **THEN** validation rejects it before the file is written

#### Scenario: an unknown provider type is a listed error

- **WHEN** `providers.toml` names a provider with `type = "oauth:unknown"`
- **THEN** parsing returns an error naming the unknown type and listing the known ones


<!-- @trace
source: provider-model-two-tier
updated: 2026-07-10
code:
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Model roles are member pools with call-time traits on the member

The model roles SHALL be exactly `main` and `cheap`. Each role SHALL be a pool with a `strategy` (`single`, `round_robin`, or `failover`) and a list of `members`, where each member is `{ provider, model, stream, modalities, effort }`. The call-time traits `stream`/`modalities`/`effort` SHALL live on the member (they follow the model/call), while `base_url`/`key`/auth live on the provider the member names. A `main` (or `cheap`) role SHALL resolve to a pool built by constructing each member from its provider's endpoint/auth plus the member's model and traits.

#### Scenario: one provider serves two models across roles

- **GIVEN** a single `api` provider `openai1`
- **WHEN** `main` has member `openai1/gpt-4o` and `cheap` has member `openai1/gpt-4o-mini`
- **THEN** both roles resolve, each routing to its own model through the same provider's endpoint and key — with no duplicated provider

#### Scenario: single strategy requires exactly one member

- **WHEN** a role with `strategy = "single"` is written with zero or more than one member
- **THEN** validation rejects it before the file is written


<!-- @trace
source: provider-model-two-tier
updated: 2026-07-10
code:
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Referential integrity is validated before write, not fail-soft

Writing `providers.toml` SHALL validate cross-references before persisting: every `member.provider` SHALL be a defined provider, and removing a provider that a role member references SHALL be refused with a message naming the referencing role. A validation failure SHALL prevent the write — the editor SHALL NOT silently drop the offending reference (unlike the runtime load path, which stays fail-soft and falls back to the env tier).

#### Scenario: a member referencing an undefined provider is rejected

- **WHEN** a role member names a provider that is not defined
- **THEN** the write is rejected and the file is left unchanged, the error naming the missing provider

##### Example: main names a provider that was never added

- **GIVEN** `providers.toml` defines provider `openai1` only
- **WHEN** a write sets `models.main` member `ghost/gpt-4o`
- **THEN** validation fails naming `ghost`, and the on-disk file still has only the pre-write `main`

#### Scenario: removing a referenced provider is refused

- **WHEN** `config provider remove openai1` runs while a role member still names `openai1`
- **THEN** the removal is refused and names the referencing role


<!-- @trace
source: provider-model-two-tier
updated: 2026-07-10
code:
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: providers.toml migrates once to the two-tier shape

A legacy `providers.toml` (provider-binds-model plus groups and a role→name map) SHALL migrate once, idempotently, to the two-tier shape. Providers that differ only by `model` (same `base_url` + `key` + auth) SHALL merge into a single provider with one member per old model; each old provider's `stream`/`modalities`/`effort` SHALL move onto its corresponding member; old roles SHALL map to `models.<role>` members (a group to a multi-member pool, a single provider to a one-member pool). Anything that cannot be migrated SHALL be reported loudly, never silently dropped. A config already in the two-tier shape SHALL NOT be re-migrated.

#### Scenario: near-duplicate providers merge into members

- **GIVEN** two legacy providers with the same `base_url` and `key` but models `gpt-4o` and `gpt-4o-mini`
- **WHEN** migration runs
- **THEN** they become one provider with two members carrying those two models, and their `stream`/`modalities`/`effort` land on the matching members

#### Scenario: migration is idempotent

- **WHEN** migration runs against a config that already has `[models.*]`
- **THEN** it makes no changes


<!-- @trace
source: provider-model-two-tier
updated: 2026-07-10
code:
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Flat model env is a bootstrap seed and broken structured config is a hard error

With no structured providers/models defined, the flat `FLEETY_MODEL_*` / `FLEETY_CHEAP_MODEL_*` environment SHALL auto-form the `main` (and `cheap`) role, so a headless/CI/Docker three-line-env deployment still runs. A `providers.toml` that is present but broken or referentially incomplete SHALL cause a hard startup error (the server refuses to boot with a clear message) instead of silently degrading to the echo stub. The echo stub SHALL survive only as the placeholder when nothing at all is configured, so a first run is still connection-verifiable.

#### Scenario: three-line env still boots

- **GIVEN** no `providers.toml`
- **WHEN** `FLEETY_MODEL_BASE_URL` and `FLEETY_MODEL` are set in the environment
- **THEN** the `main` role is formed from them and the server serves that model

#### Scenario: a broken structured config refuses to boot

- **WHEN** `providers.toml` exists but has a role member referencing an undefined provider
- **THEN** the server exits with a clear error rather than silently running the echo stub

<!-- @trace
source: provider-model-two-tier
updated: 2026-07-10
code:
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->