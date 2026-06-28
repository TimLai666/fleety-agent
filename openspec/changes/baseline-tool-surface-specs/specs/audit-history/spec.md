## ADDED Requirements

### Requirement: List recent audit entries

The system SHALL provide a `history_list` tool that returns recent audit-log entries (tool calls, results, replies) for the current device, with an optional `limit` (default 20). Every tool call SHALL be recorded to the per-device audit log so any mutating action can be explained after the fact.

#### Scenario: limit the returned entries

- **WHEN** `history_list` is called with `limit=5` and the log has more than 5 entries
- **THEN** at most the 5 most recent entries are returned
