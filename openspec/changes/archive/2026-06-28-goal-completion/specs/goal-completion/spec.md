## ADDED Requirements

### Requirement: The agent self-sets a goal and an optional checklist

The system SHALL provide `set_goal({goal, steps?})` for the agent to record its own goal (derived from the user's request and context) plus an optional self-managed checklist, `complete_step({step})` to mark one step done, and `goal_status()` to report the goal and which steps are done or pending. `set_goal` MAY be called again to revise the plan.

#### Scenario: setting a goal activates the mechanism

- **WHEN** the agent calls `set_goal` with a non-empty goal
- **THEN** the goal becomes active and `goal_status` reports it with no terminal state yet

### Requirement: Drive to the goal until a terminal signal

After a turn ends, if a goal is active and the agent has called neither `complete_goal` nor `ask_user`, the runtime SHALL treat the stop as premature and run another turn, injecting a continuation nudge naming the goal and the pending steps. The two terminal signals SHALL be `complete_goal({summary?})` (the goal is achieved) and `ask_user({question})` (a question the user must answer). When no goal is active, a turn that ends SHALL complete normally (single-shot).

#### Scenario: a premature stop is re-engaged; a terminal signal stops it

- **WHEN** a turn ends with an active goal and neither terminal signal was called
- **THEN** the runtime runs another turn toward the goal
- **WHEN** the agent calls `complete_goal` or `ask_user`
- **THEN** the runtime stops the loop after that turn

### Requirement: Auto-continuation is bounded

The number of automatic continuations per user message SHALL be capped (configurable via `FLEETY_GOAL_MAX_CONTINUES`, floor 1). On reaching the cap the runtime SHALL stop and tell the user the cap was hit and the goal may be incomplete, never looping unbounded.

#### Scenario: the cap stops a non-converging loop

- **WHEN** the auto-continuation count reaches the configured cap without a terminal signal
- **THEN** the runtime stops and reports that the cap was reached

### Requirement: Only the terminal turn replies, and speaks

Intermediate continuation turns SHALL NOT emit a terminal user-facing reply; only the turn that calls `complete_goal` or `ask_user` SHALL produce the user-facing reply, and (when voice mode is on) the spoken summary. Progress MAY still stream as deltas during intermediate turns.

#### Scenario: voice fires only at completion or a required question

- **WHEN** the goal loop runs several intermediate continuation turns and then completes
- **THEN** the user receives one final reply (and one spoken summary when voice is on), not one per intermediate turn

### Requirement: Goal tools are top-level only and the core stays host-free

The five goal tools SHALL be registered only on a top-level registry, so a subagent's registry omits them and a subagent cannot alter its parent's goal. The goal state and tools SHALL live in agent-core and depend on no host crate.

#### Scenario: a subagent has no goal tools

- **WHEN** a subagent's tool registry is built
- **THEN** it contains none of `set_goal`, `complete_goal`, `ask_user`
