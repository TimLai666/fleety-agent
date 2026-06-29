## ADDED Requirements

### Requirement: Named providers and groups are defined in a separate file

The system SHALL load a `providers.toml` (default `~/.fleety/providers.toml`, overridable) defining any number of named providers and groups, separate from the main config so keys stay isolated. Each provider entry SHALL carry a name plus its base URL, model, key, and optional stream / modalities / effort; the provider type SHALL be derived from the base URL as today. A group SHALL list member provider names and a strategy (`round_robin` or `failover`). A roles table SHALL map role names (e.g. `main`, `cheap`, or any subagent tier name) to a provider or group name. Parsing SHALL be a pure function; a missing or unparseable file SHALL fail soft (treated as absent) and never crash.

#### Scenario: providers and groups parse

- **WHEN** `providers.toml` defines two providers and a `round_robin` group over them, with `main` mapped to that group
- **THEN** parsing yields those two providers, the group with its strategy, and the role mapping

#### Scenario: a broken file falls back, not crashes

- **WHEN** `providers.toml` exists but is not valid TOML
- **THEN** it is treated as absent (env fallback applies) and the server still starts

### Requirement: A group pools members with round-robin or failover

A group SHALL be exposed as a single model provider that dispatches a call across its members. On any member error (after that member's own internal retries), it SHALL try the next member, and only return an error once all members have failed (returning the last error; it SHALL NOT panic). `round_robin` SHALL advance the starting member per call to spread load; `failover` SHALL always start at the first member and only advance on failure. The attempt-order computation SHALL be a pure function of (start, member count).

#### Scenario: failover to the next member on error

- **WHEN** a 2-member group's first member errors and the second succeeds
- **THEN** the call returns the second member's success rather than an error

#### Scenario: all members fail

- **WHEN** every member of a group errors
- **THEN** the call returns the last error and does not panic

#### Scenario: round-robin spreads calls

- **WHEN** consecutive calls hit a healthy `round_robin` group of N members
- **THEN** the starting member advances each call (calls are spread across members)

##### Example: attempt order

| strategy | start | members | attempt order |
|---|---|---|---|
| round_robin (call 1) | 0 | [a,b,c] | a, b, c |
| round_robin (call 2) | 1 | [a,b,c] | b, c, a |
| failover | 0 | [a,b,c] | a, b, c |

### Requirement: Roles resolve by name with an env fallback

Resolving a role or tier name SHALL look it up in the roles map, then by provider/group name, yielding the corresponding provider (a single provider or a group pool). An unknown name SHALL resolve to `main`. When no `providers.toml` is present (or it is empty), the system SHALL fall back to the existing environment configuration, building providers named `main` and `cheap` from `FLEETY_MODEL_*` / `FLEETY_CHEAP_MODEL_*`, so the zero-config behavior is unchanged. A subagent's tier name SHALL be able to reference any defined provider or group.

#### Scenario: zero-config is unchanged

- **WHEN** no `providers.toml` is present
- **THEN** `main` and `cheap` are built from the environment exactly as before this change

#### Scenario: a subagent tier names a pool

- **WHEN** `providers.toml` defines a group `codex` and a subagent is spawned with tier `codex`
- **THEN** the subagent runs on that group (pooled across its members)

### Requirement: A pooled provider reports homogeneous capabilities

A group's pooled provider SHALL assume its members are the same model across accounts/endpoints: it SHALL report the first member's input-modality capabilities, and applying a reasoning effort SHALL produce a pool whose members each carry that effort.

#### Scenario: capabilities come from the first member

- **WHEN** a group's pooled provider is asked for its capabilities
- **THEN** it reports the first member's modality capabilities
