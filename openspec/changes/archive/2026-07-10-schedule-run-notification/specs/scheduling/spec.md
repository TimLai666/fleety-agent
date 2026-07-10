## ADDED Requirements

### Requirement: Record each run's outcome

The scheduler SHALL write a `last_outcome` record onto a schedule after every unattended run, for both successful and failed runs. The record MUST contain a `status` (`ok` or `error`), a one-line `summary` (the truncated final assistant output on success, the truncated error report on failure), and a `ts` (unix seconds). A run whose `run_turn` returns an error MUST be isolated: the scheduler MUST record an `error` outcome, mark the schedule fired, and continue to the remaining due schedules; a single failing schedule SHALL NOT abort the tick and SHALL NOT be retried on every subsequent tick.

#### Scenario: successful run records an ok outcome

- **WHEN** a due schedule's unattended run completes successfully during a tick
- **THEN** the schedule's `last_outcome.status` is `ok`, `summary` reflects the run's output, and the schedule is marked fired

#### Scenario: a failing schedule is recorded and isolated

- **WHEN** one due schedule's run fails while another due schedule in the same tick succeeds
- **THEN** both schedules are marked fired, each gets a `last_outcome` with the matching `status` (`error` and `ok`), and the failed schedule's `at:` trigger is not due on the next tick

### Requirement: Surface last run outcome in schedule_list

`schedule_list` SHALL include each schedule's `last_run` and `last_outcome` (when the schedule has run at least once) so a user can see when it last ran and whether it succeeded or failed.

#### Scenario: schedule_list shows the last outcome

- **WHEN** a schedule has been run and `schedule_list` is called
- **THEN** the listing entry for that schedule includes `last_run` and a `last_outcome` carrying its `status`, `summary`, and `ts`

### Requirement: Proactively notify the owner on next connect

When a device owned by the scheduler's user connects, the server SHALL deliver, as `ServerMsg::Assistant` messages, each schedule outcome completed since it was last notified, with failures prominently marked, and SHALL advance each delivered schedule's notification watermark so the same outcome is not delivered twice. The server SHALL NOT deliver these notifications to a Guest connection or to a device whose acting user differs from the scheduler's owner.

#### Scenario: owner receives unnotified outcomes once

- **WHEN** an owner device connects while a schedule has an outcome newer than its notification watermark
- **THEN** the owner receives one assistant message for that outcome (a failure is clearly marked and points at `schedule-<id>`), and a subsequent connect from that device does not redeliver the same outcome

#### Scenario: non-owner connection receives no schedule notifications

- **WHEN** a Guest connection (or a device whose acting user is not the scheduler's owner) connects with schedule outcomes pending
- **THEN** no schedule-outcome assistant messages are delivered to that connection
