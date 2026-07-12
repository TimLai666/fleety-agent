# per-task-effort Specification

## Purpose

TBD - created by archiving change 'per-task-effort'. Update Purpose after archive.

## Requirements

### Requirement: Model calls carry an optional reasoning effort

A model call SHALL accept an optional reasoning effort (low / medium / high). A provider SHALL translate it to the model family's field — `reasoning_effort` for OpenAI-compatible endpoints, the thinking setting for Gemini — and SHALL omit any effort field when the model does not support one, so passing effort never by itself causes the call to fail. The mapping from (scheme, effort) to a request field SHALL be a pure function.

#### Scenario: effort sent to a supporting model

- **WHEN** a call with effort=high targets an OpenAI-compatible model whose scheme is reasoning-capable
- **THEN** the request body includes `reasoning_effort: "high"`

#### Scenario: effort omitted for a non-supporting model

- **WHEN** a call with effort=high targets a model whose effort scheme is none
- **THEN** the request body contains no effort/reasoning field and the call is not rejected for carrying effort

##### Example: (scheme, effort) → field

| scheme | effort | request field |
|---|---|---|
| OpenAiReasoning | high | reasoning_effort = "high" |
| OpenAiReasoning | low | reasoning_effort = "low" |
| GeminiThinking | medium | thinking setting (medium) |
| None | high | (no field emitted) |


<!-- @trace
source: per-task-effort
updated: 2026-06-29
code:
  - docs/env.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-workflow/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/effort.rs
  - crates/agent-core/src/model.rs
-->

---
### Requirement: The main agent sets its own effort dynamically

The main agent SHALL be able to change the reasoning effort applied to its own subsequent turns via a tool accepting `low`, `medium`, `high`, or `auto`. A `low`/`medium`/`high` value SHALL be recorded as a manual pin that persists at the conversation/session level. The runtime SHALL re-read the current effort before each turn it drives for a request — INCLUDING each goal-continuation turn within the same request — so that a change made mid-request takes effect on the next continuation turn rather than being deferred to the next user message. An `auto` value SHALL clear the manual pin, returning control to automatic difficulty selection (or the configured default when automatic selection is disabled). The agent's own effort SHALL NOT be set by a subagent.

#### Scenario: mid-request change applies to the next continuation turn

- **WHEN** the agent invokes the set-effort tool with high while a multi-turn goal-driven request is still running
- **THEN** the next goal-continuation turn of that same request issues its model calls with effort=high, without waiting for a new user message

#### Scenario: manual pin persists across turns

- **WHEN** the agent sets effort=high and does not change it
- **THEN** subsequent turns keep issuing model calls at effort=high until the agent changes the value again

#### Scenario: auto clears the manual pin

- **WHEN** the agent invokes the set-effort tool with auto
- **THEN** the manual pin is cleared and later turns fall back to automatic difficulty selection or the configured default


<!-- @trace
source: dynamic-effort
updated: 2026-07-12
code:
  - crates/fleety-tools/src/config.rs
  - docs/env.md
  - crates/fleety-server/src/effort.rs
  - docs/tools.md
  - crates/fleety-server/src/conn.rs
  - prompts/protocol.md
  - prompts/rules.md
-->

---
### Requirement: A subagent's effort is decided by the spawning agent

When an agent spawns a subagent, the spawn request SHALL accept an effort chosen by the spawning (parent) agent; when omitted it SHALL inherit a default. The subagent SHALL run all of its turns at that effort and SHALL NOT have a means to change its own effort.

#### Scenario: parent spawns a low-effort subagent

- **WHEN** an agent spawns a subagent with effort=low
- **THEN** the subagent's model calls all use effort=low, and the subagent has no tool to alter its own effort

#### Scenario: spawn without effort inherits the default

- **WHEN** an agent spawns a subagent without specifying effort
- **THEN** the subagent uses the configured default effort


<!-- @trace
source: per-task-effort
updated: 2026-06-29
code:
  - docs/env.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-workflow/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/effort.rs
  - crates/agent-core/src/model.rs
-->

---
### Requirement: Default effort is configurable per tier

A default reasoning effort SHALL be configurable per tier via `FLEETY_MODEL_EFFORT` and `FLEETY_CHEAP_MODEL_EFFORT`, registered in the typed config registry. When unset, calls SHALL carry no effort (endpoint default), preserving prior behavior.

#### Scenario: unset default preserves prior behavior

- **WHEN** no effort default is configured and neither the agent nor a parent specifies one
- **THEN** model calls carry no effort field, exactly as before this change

<!-- @trace
source: per-task-effort
updated: 2026-06-29
code:
  - docs/env.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-workflow/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/effort.rs
  - crates/agent-core/src/model.rs
-->

---
### Requirement: Effort is auto-selected by task difficulty

When automatic effort is enabled and no manual pin is active, the runtime SHALL classify each incoming non-empty top-level user message by difficulty before the first model inference and apply the resulting effort (low / medium / high) to that turn, so a hard task starts at a higher effort without relying on the agent to raise it. The classification SHALL use a lightweight model call and SHALL degrade gracefully: a failed or unparseable classification SHALL yield no change (the configured default applies) and SHALL NOT reject or block the turn. The automatically selected effort SHALL NOT be recorded as a manual pin, so it does not persist across messages of its own accord. Automatic selection SHALL be controlled by `FLEETY_AUTO_EFFORT`, registered in the typed config registry. The effective effort for any turn SHALL be the manual pin when set, otherwise the automatically selected effort, otherwise the configured default.

#### Scenario: a hard message starts at high effort

- **WHEN** automatic effort is enabled, no manual pin is set, and a difficult request arrives
- **THEN** the classifier selects high and the turn's first model inference already runs at effort=high

#### Scenario: a manual pin overrides automatic selection

- **WHEN** a manual pin of low is active and a message the classifier would rate high arrives
- **THEN** the turn runs at effort=low because the manual pin wins, and automatic selection is not consulted

#### Scenario: classification failure falls back to the default

- **WHEN** the classification call fails or returns an unparseable answer
- **THEN** the turn proceeds at the configured default effort with no error surfaced

#### Scenario: disabled by configuration

- **WHEN** FLEETY_AUTO_EFFORT is off
- **THEN** no classification call is made and only a manual pin or the configured default determines effort

<!-- @trace
source: dynamic-effort
updated: 2026-07-12
code:
  - crates/fleety-tools/src/config.rs
  - docs/env.md
  - crates/fleety-server/src/effort.rs
  - docs/tools.md
  - crates/fleety-server/src/conn.rs
  - prompts/protocol.md
  - prompts/rules.md
-->