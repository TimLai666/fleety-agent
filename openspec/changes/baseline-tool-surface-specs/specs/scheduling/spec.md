## ADDED Requirements

### Requirement: Schedule prompts to run later

The system SHALL provide `schedule_create`, `schedule_list`, and `schedule_delete`. `schedule_create` SHALL accept a `trigger` (one-shot timestamp or recurring cron with optional `tz`) and a `prompt`, and SHALL accept an optional `mandate` and `allowed_tools` captured at creation time. `schedule_list` SHALL show each schedule's timezone and next fire time. The fire loop SHALL run a schedule only when the current time matches its trigger.

#### Scenario: cron schedule reports its next fire time

- **WHEN** `schedule_create` registers a cron trigger with a `tz` and `schedule_list` is called
- **THEN** the listing shows that schedule's timezone and computed next fire time
