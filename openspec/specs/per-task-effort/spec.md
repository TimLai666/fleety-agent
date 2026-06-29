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

The main agent SHALL be able to change the reasoning effort applied to its own subsequent turns via a tool. The chosen effort SHALL persist at the conversation/session level and be applied to each later model call until the agent changes it again. The agent's own effort SHALL NOT be set by a subagent.

#### Scenario: agent raises its own effort mid-conversation

- **WHEN** the agent invokes the set-effort tool with high
- **THEN** its next and subsequent turns issue model calls with effort=high until it changes the value again


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