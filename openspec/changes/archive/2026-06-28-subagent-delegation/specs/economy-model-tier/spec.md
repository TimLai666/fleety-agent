## ADDED Requirements

### Requirement: Optional second economy provider

The runtime SHALL read `FLEETY_CHEAP_MODEL_BASE_URL`, `FLEETY_CHEAP_MODEL`, `FLEETY_CHEAP_MODEL_KEY`, and `FLEETY_CHEAP_MODEL_STREAM` with the same semantics as the corresponding `FLEETY_MODEL_*` variables, and SHALL build a second provider when both `FLEETY_CHEAP_MODEL_BASE_URL` and `FLEETY_CHEAP_MODEL` are set. The cheap provider MAY use a different provider implementation and model than the main one.

#### Scenario: a configured cheap model builds a distinct provider

- **WHEN** `FLEETY_CHEAP_MODEL_BASE_URL` and `FLEETY_CHEAP_MODEL` are both set to values different from the main model
- **THEN** the runtime builds a separate cheap-tier provider using those values

### Requirement: Tier resolution and fallback

The `main` tier SHALL resolve to the main provider and the `cheap` tier SHALL resolve to the cheap provider. When the cheap model is not configured, the `cheap` tier SHALL resolve to the main provider (an alias of the same provider), so selecting `cheap` always yields a valid provider and never errors.

#### Scenario: cheap falls back to main when unset

- **WHEN** the cheap model variables are unset and a subagent requests `model="cheap"`
- **THEN** the subagent runs on the main provider without error
