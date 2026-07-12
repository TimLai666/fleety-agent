## MODIFIED Requirements

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

## ADDED Requirements

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
