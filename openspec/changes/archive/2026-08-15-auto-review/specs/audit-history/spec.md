## MODIFIED Requirements

### Requirement: List recent audit entries

The system SHALL provide a `history_list` tool that returns recent audit-log entries (tool calls, results, replies, and auto-review decisions) for the current device, with an optional `limit` (default 20). Every tool call and every auto-review decision SHALL be recorded to the per-device audit log so any mutating action and its authorization outcome can be explained after the fact. Auto-review entries SHALL include the decision, risk class, tool, reviewer provider/model label, danger-signal codes, latency, and sanitized reason, and SHALL omit raw arguments, prompt text, tokens, API keys, and passwords.

#### Scenario: limit the returned entries

- **WHEN** `history_list` is called with `limit=5` and the log has more than 5 entries
- **THEN** at most the 5 most recent entries are returned

#### Scenario: audit contains an auto-review decision without secrets

- **WHEN** an auto-reviewed candidate is approved or denied
- **THEN** `history_list` contains its sanitized decision metadata and contains no raw secret or candidate argument
