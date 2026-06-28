## ADDED Requirements

### Requirement: Asynchronous task registry and state machine

The runtime SHALL maintain a registry mapping each subagent `task_id` to its task record (mode, tier, messages, output, and lifecycle state). A task SHALL move through the states Spawned, Running, then a terminal Done, Failed, or Stopped. A foreground subagent SHALL be awaited inline; a background subagent SHALL run on a separate task whose handle is retained in the registry.

#### Scenario: background task is tracked through completion

- **WHEN** a background `spawn_subagent` starts
- **THEN** the registry holds its `task_id` in the Running state and transitions it to Done (or Failed) when the subagent finishes

### Requirement: Background completion notification with de-duplication

When a background subagent reaches a terminal state, the runtime SHALL emit a user-facing notification AND proactively wake a parent coordinator turn seeded with the completion notice, so the parent synthesizes the result without waiting for the next user message. Each completion SHALL be delivered to the parent at most once (de-duplicated by `task_id` via a delivered flag), and multiple near-simultaneous completions MAY be batched into a single wake.

#### Scenario: a completed background subagent wakes the parent exactly once

- **WHEN** a background subagent reaches a terminal state
- **THEN** the runtime starts a parent coordinator turn seeded with exactly one completion notice for that `task_id`, and that notice is not delivered again on later turns

### Requirement: Continue and stop subagents

The system SHALL provide `send_subagent_message` (continue an existing non-running subagent with a new `prompt`, preserving its messages) and `stop_subagent` (cancel a subagent, aborting a background task's handle). `subagent_status` SHALL report a task's current state and, when finished, its output. Operating on an unknown `task_id`, or sending to a still-running subagent, SHALL return an actionable error.

#### Scenario: stop transitions to Stopped

- **WHEN** `stop_subagent` is called on a running background subagent
- **THEN** its background handle is aborted and its state becomes Stopped

### Requirement: Non-interactive gate and concurrency limit

Background subagents SHALL run under a non-interactive approval gate: under `full_access` their mutating tools run; under `require_approval` they are limited to read tools unless `allowed_tools` pre-grants specific tools at spawn time. The number of concurrently running subagents SHALL be capped (configurable, floor 1); a spawn that would exceed the cap SHALL fail with an actionable error rather than queue silently. Every subagent action SHALL be recorded to the parent device's audit log, tagged with the subagent `task_id`.

#### Scenario: spawning past the concurrency cap is refused

- **WHEN** a `spawn_subagent` would exceed the configured concurrency cap
- **THEN** the call fails with an actionable error naming the cap
