## ADDED Requirements

### Requirement: The model is shown a resident tool set, not the whole surface

A conversation SHALL open with a resident set of tools plus a tool-search entry point, rather than every registered tool. The remaining tools SHALL NOT appear in the request until they are activated.

The resident set SHALL cover the capabilities nearly every conversation needs — workspace files and command execution, the skills entry points, core memory read and write, and the cross-device entry points — so that a typical conversation needs no search at all.

The tool-search entry point SHALL always be resident and SHALL NOT be deactivatable, because it is the only way the model can discover anything else.

#### Scenario: a new conversation opens with the resident set

- **WHEN** a conversation makes its first model call
- **THEN** the tools shown are the resident set plus the tool-search entry point, and no other registered tool appears

#### Scenario: search is never hidden

- **WHEN** any activation state is in effect, including one loaded from storage
- **THEN** the tool-search entry point is still shown

### Requirement: The model can discover and activate further tools by capability

The system SHALL provide a tool that takes a capability description and returns the matching group's name together with each tool in it and a one-line summary, and SHALL add that group to the conversation's activated set.

Activation SHALL be by group rather than by individual tool, because a single piece of work normally needs several tools from the same group and per-tool activation would cost a round trip each.

A query that matches nothing SHALL say so plainly and SHALL leave the activated set unchanged. Re-querying an already-activated group SHALL neither duplicate it nor raise an error.

#### Scenario: a matching search activates the group

- **WHEN** the model searches for a capability that a group provides
- **THEN** the result names that group and its tools, and the group becomes activated

##### Example: reaching browser control

- **GIVEN** a conversation whose activated set is empty
- **WHEN** the model searches for "control a web browser"
- **THEN** the browser group is named in the result and becomes activated, so the following model call is shown its navigate, evaluate, screenshot, open and close tools

#### Scenario: a search that matches nothing changes nothing

- **WHEN** the model searches for a capability no group provides
- **THEN** the result states that nothing matched, and the activated set is unchanged

#### Scenario: re-activating is harmless

- **WHEN** the model searches for a capability whose group is already activated
- **THEN** the result is returned normally and the activated set is unchanged

### Requirement: Activation takes effect within the same turn

A group activated during a turn SHALL be visible to the next model call of that same turn. The set of tools offered to the model SHALL therefore be read afresh for each model call rather than captured once when the turn begins.

Without this, the model would discover a capability and then be unable to use it until a later turn, which would make discovery useless in the case it exists for.

#### Scenario: a tool activated mid-turn is usable in the same turn

- **WHEN** the model activates a group at one step of a turn
- **THEN** the next model call in that turn is shown that group's tools and may call them

### Requirement: Activation persists for the conversation

The activated set SHALL be stored with the conversation and restored with it, so a group activated once stays available across later turns and across a restart.

An activated set that cannot be read or parsed SHALL be treated as unset rather than as an error. An activated name that matches no registered tool SHALL be ignored, so that state written by a different version cannot break a conversation.

#### Scenario: activation survives later turns and restart

- **WHEN** a group is activated and the conversation continues, including after a restart
- **THEN** that group's tools are still shown without searching again

#### Scenario: unreadable or stale activation state degrades safely

- **WHEN** the stored activated set cannot be parsed, or names a tool that is not registered
- **THEN** the conversation proceeds — an unparsable set is treated as unset, and an unknown name is ignored

### Requirement: Activation is a context budget, never an authorization boundary

Whether a tool is activated SHALL affect only which tools are shown to the model. It SHALL NOT be consulted when a tool call is executed: execution eligibility remains governed by the tool's risk class and the approval gate.

Treating activation as authorization would create a second, weaker source of permission alongside the approval gate, and would produce spurious refusals in the window between activating a group and the model being shown it.

#### Scenario: execution is gated by risk, not by activation

- **WHEN** a registered tool is called
- **THEN** the call is admitted or refused by its risk class and the approval gate, and the activated set is not consulted

### Requirement: An unset activation state preserves existing behavior

A tool registry with no activation state set SHALL offer every registered tool, exactly as before this capability existed. Callers that never set one — subagents, workflows, the offline evaluator, the device daemon — SHALL be unaffected.

#### Scenario: no activation state means every tool

- **WHEN** a registry has no activation state set
- **THEN** it offers every registered tool
