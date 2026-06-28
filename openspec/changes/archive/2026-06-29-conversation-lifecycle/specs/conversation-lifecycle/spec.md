## ADDED Requirements

### Requirement: Conversations can roll over without losing history

The agent SHALL be able to roll a device's conversation over: a fresh conversation becomes active while the previous one is preserved (not deleted) and chained via a successor link, so it remains searchable through conversation recall. Rollover SHALL be per-device. Front-ends SHALL be informed via an additive server message; a client that does not handle it SHALL still work, with the server transparently redirecting further messages to the successor.

#### Scenario: rollover sets aside, does not delete

- **WHEN** the agent rolls the conversation over
- **THEN** a new conversation becomes active, the previous conversation is preserved and linked as its predecessor, and the previous conversation is still searchable via recall

#### Scenario: older client keeps working after rollover

- **WHEN** a client that does not understand the rollover message keeps sending the previous conversation id
- **THEN** the server transparently routes those messages to the successor conversation

### Requirement: Rollover is agent-judged, triggered explicitly or by a silent nudge

Rollover SHALL be decided by the agent, never forced by raw length. The agent SHALL have an explicit tool to roll over when it judges a task or topic complete. In addition, after a goal completes (and when context pressure is high) the system SHALL raise an implicit, out-of-band nudge that prompts the agent to consider distilling and rolling over. The nudge and any resulting housekeeping SHALL run silently and SHALL NOT produce user-facing system-style narration.

#### Scenario: explicit rollover by tool

- **WHEN** the agent calls the rollover tool after finishing a task
- **THEN** the conversation rolls over and the agent continues in the fresh conversation

#### Scenario: implicit nudge after goal completion is silent

- **WHEN** a goal completes and the system raises the rollover/distill nudge
- **THEN** the agent decides whether to act, and the user sees only normal answers — never a "the system asked me to roll over" style message

#### Scenario: length only nudges, never forces

- **WHEN** the conversation is long enough to be under context pressure
- **THEN** the agent is nudged to consider rollover but is not switched automatically

### Requirement: Takeaways are distilled into the right memory layer by kind

When rolling over (or when distilling), the agent SHALL route each worthwhile takeaway to the appropriate layer by its kind: durable knowledge or insight to the wiki, pending work to TODO, user facts to USER, device-operational facts to the device notes, and ephemeral recap to nowhere (conversation recall already preserves it). Distillation SHALL use the existing memory/wiki/device-note tools; the wiki SHALL hold wisdom, not raw conversation summaries.

#### Scenario: a durable insight goes to the wiki, a task goes to TODO

- **WHEN** the agent distills a conversation that produced a reusable insight and an unfinished task
- **THEN** the insight is written to the wiki and the task is recorded in TODO, each via the existing tools

#### Scenario: ephemeral chatter is not written to memory

- **WHEN** the conversation's content is only ephemeral recap with nothing durable
- **THEN** nothing is written to the memory layers, since recall already preserves the conversation

### Requirement: Housekeeping never blocks the user

After a reply has been delivered, post-turn housekeeping — skill reflection, memory distillation, and conversation rollover — SHALL run in the background, off the connection's message loop, so the user's next message is handled without waiting for it. Housekeeping SHALL use the economy model tier rather than the main model, and SHALL be single-flight per conversation (a second concurrent housekeeping for the same conversation is skipped). A housekeeping failure SHALL be logged and SHALL NOT affect the live conversation.

#### Scenario: the next message is not blocked by housekeeping

- **WHEN** a turn finishes and background housekeeping starts, and the user immediately sends another message
- **THEN** the new message is handled without waiting for the housekeeping to complete

#### Scenario: housekeeping uses the economy tier and is single-flight

- **WHEN** housekeeping runs for a conversation
- **THEN** it uses the economy model tier, and a second housekeeping for the same conversation while one is in flight is skipped rather than queued

#### Scenario: housekeeping failure does not affect the conversation

- **WHEN** a background housekeeping task fails
- **THEN** the failure is logged and the live conversation is unaffected, with nothing surfaced to the user
