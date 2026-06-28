## ADDED Requirements

### Requirement: Reflection fires after a complex task

After a user message's goal turn completes, when the message's accumulated tool-step count reaches the configured threshold (`FLEETY_SKILL_REFLECT_MIN_STEPS`), the system SHALL run exactly one reflection turn that lets the agent persist what it learned. The step count SHALL be the sum of the per-turn provider step counts for that message.

#### Scenario: a complex message triggers one reflection turn

- **WHEN** a user message's goal completes with a tool-step count at or above the threshold
- **THEN** the runtime runs one additional reflection turn after the goal's terminal reply

##### Example: threshold decision (threshold = 5)

| Message tool-steps | Reflection turn runs? |
| ------------------ | --------------------- |
| 7                  | yes                   |
| 5                  | yes                   |
| 3                  | no                    |

### Requirement: Reflection is bounded and configurable

The reflection turn SHALL run at most once per user message and SHALL NOT itself trigger another reflection (no recursion). The threshold SHALL be configurable via `FLEETY_SKILL_REFLECT_MIN_STEPS`; a value of `0` SHALL disable reflection entirely, and below-threshold messages SHALL run no reflection turn and spend no extra tokens.

#### Scenario: disabled or below threshold runs nothing

- **WHEN** the threshold is `0`, or the message's tool-step count is below the threshold
- **THEN** no reflection turn runs and behaviour is identical to a plain completed message

#### Scenario: reflection does not recurse

- **WHEN** a reflection turn itself uses several tools
- **THEN** it still triggers no further reflection turn

### Requirement: Reflection captures procedures, facts, or nothing

In the reflection turn the agent SHALL save or update a reusable procedure as an authored skill, SHALL record durable user or project facts to memory or the knowledge wiki, and SHALL persist nothing when nothing is worth keeping. A saved skill MAY include a helper tool script under `scripts/` referenced from its `SKILL.md`.

#### Scenario: reusable procedure becomes an authored skill

- **WHEN** the completed work contains a reusable procedure worth keeping
- **THEN** the agent writes or updates an authored skill capturing it

#### Scenario: nothing worth keeping writes nothing

- **WHEN** the completed work has no reusable procedure and no durable fact
- **THEN** the reflection turn saves no skill, memory, or wiki entry

### Requirement: Skill-held tools run via the command tool

A tool script a skill stores under `scripts/` SHALL be runnable by the agent through the existing command-execution tool, using the skill's directory path that `use_skill` returns to build the script's path. No dedicated skill-script execution tool is introduced.

#### Scenario: agent runs a skill's bundled script

- **WHEN** a loaded skill references a script under its `scripts/` directory
- **THEN** the agent runs it through the command-execution tool using the path returned by `use_skill`
