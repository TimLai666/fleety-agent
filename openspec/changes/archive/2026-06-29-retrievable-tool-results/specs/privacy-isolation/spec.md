## ADDED Requirements

### Requirement: Tool-result retrieval and audit listing respect the user boundary

Retrieving a tool result (`fetch_tool_result`) and listing audit history
(`history_list`) SHALL be confined to conversations the acting user can access.
An id that exists but belongs to another user's conversation SHALL be reported as
not found, with no indication that it exists (consistent with the user-as-privacy-
boundary, no-leak rule). The audit listing SHALL return only entries from the
acting user's accessible conversations.

#### Scenario: cannot fetch another user's tool result

- **WHEN** acting user A calls `fetch_tool_result` with an id from user B's conversation
- **THEN** it is reported as not found, with no hint that the id exists

#### Scenario: audit listing is scoped to the acting user

- **WHEN** the acting user lists audit history on a shared device
- **THEN** only that user's accessible entries are returned, not other users' tool output
