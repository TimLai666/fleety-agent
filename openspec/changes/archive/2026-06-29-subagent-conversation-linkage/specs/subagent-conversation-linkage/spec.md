## ADDED Requirements

### Requirement: A subagent run is a retrievable, user-owned child conversation

A subagent run SHALL be recorded as a child conversation, identified by an id
derived from its task id and owned by the parent turn's acting user. Its
transcript SHALL be stored like a normal conversation so it is retrievable
(recall / listing), and its events SHALL be conversation-tagged with the child id
so its tool output is reachable by tool-result retrieval and bounded by the user
privacy scope. A subagent spawned by a guest (no identified user) SHALL be
unowned, consistent with a guest's own conversations.

#### Scenario: the subagent's record is retrievable and owned by the spawning user

- **WHEN** a parent turn acting as a user spawns a subagent
- **THEN** the subagent's run is stored as a child conversation owned by that user, retrievable like other conversations

#### Scenario: the subagent's tool output is fetchable and scoped

- **WHEN** the agent later retrieves a tool result produced inside the subagent run
- **THEN** it is reachable (the events are tagged to the child conversation) and only within the owning user's scope

### Requirement: The parent conversation links to its subagent children

The parent conversation SHALL carry an explicit link to each subagent's child
conversation id, recorded at the spawn and at completion, and the system SHALL
keep a parent→children index so a conversation can enumerate the subagents it
spawned and open each one's full record. The parent's inline result summary SHALL
be retained.

#### Scenario: navigate from a conversation to its subagents

- **WHEN** asking which subagents a conversation spawned
- **THEN** the parent→children index returns the child conversation ids, each opening that subagent's full record

#### Scenario: the spawn carries the child id

- **WHEN** a subagent is spawned
- **THEN** the spawn result references the child conversation id, and the completion seed names it
