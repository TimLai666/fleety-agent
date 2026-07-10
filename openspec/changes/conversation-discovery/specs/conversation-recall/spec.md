## ADDED Requirements

### Requirement: Users can discover resumable conversations from the CLI

The system SHALL expose a user-facing wire request that lists the connecting device's recent conversations, each carrying its conversation id, last-activity time (`last_ts_secs`), event count, and a short preview drawn from the conversation's first user message. The listing SHALL be scoped to the acting user resolved for that device, falling back to the device's own unattributed conversations when the device has no owner, and SHALL NOT include conversations owned by any other user. The `fleety` CLI SHALL provide a `conversations` command that issues this request and renders the results most-recent-first with a relative last-activity time and the preview, so a user can find the conversation id that `fleety resume` needs. The request and its reply SHALL be additive on the wire: a server or client that does not understand them SHALL continue to work unchanged.

#### Scenario: listing returns newest-first with previews

- **WHEN** a user runs `fleety conversations` on a device whose owner has past conversations
- **THEN** the CLI prints each conversation's id, a relative last-activity time, and a first-message preview, ordered most-recent first

#### Scenario: discovery feeds resume

- **WHEN** a user takes a conversation id printed by `fleety conversations` and runs `fleety resume <id>`
- **THEN** that conversation's events are replayed, so the listing is a working entry point to resume

#### Scenario: listing is scoped to the acting user

- **WHEN** the conversation listing is produced for a device
- **THEN** only the acting user's (device owner's) conversations are returned, and a different user's conversations MUST NOT appear

#### Scenario: empty listing is honest

- **WHEN** the device's owner has no stored conversations
- **THEN** the command reports that there are none and exits successfully rather than erroring
