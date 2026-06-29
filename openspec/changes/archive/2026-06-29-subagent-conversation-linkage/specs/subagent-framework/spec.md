## ADDED Requirements

### Requirement: The host records a subagent under a parent-owned child conversation

The subagent host SHALL persist a subagent run's events under a child conversation
(tagged with the child conversation id), owned by the parent turn's acting user,
rather than as untagged device-audit events, and SHALL record a parent→child link.
The core subagent mechanism, the one-level nesting cap, and the manager lifecycle
are unchanged; only the host's persistence and ownership change.

#### Scenario: events are tagged to the child, not untagged audit

- **WHEN** the host records a subagent run's events
- **THEN** they are written tagged to the child conversation id (not as untagged device-audit entries)

#### Scenario: ownership follows the parent's acting user

- **WHEN** the host persists the subagent's child conversation
- **THEN** its owner is the parent turn's acting user, not the device owner
