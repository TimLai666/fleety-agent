## MODIFIED Requirements

### Requirement: ACP methods map to the fleety-server agent

The adapter SHALL bridge ACP to the existing fleety-server rather than reimplementing the agent. It SHALL handle `initialize` (version + capability negotiation), `session/new`, `session/load`, `session/prompt`, and `session/cancel`, translating them to the server's conversation protocol and streaming the server's assistant output back as `session/update` notifications — each tagged by `sessionUpdate: "agent_message_chunk"` and carrying a text content block, as ACP editors require. `session/cancel` SHALL be translated to the server's `CancelTurn` frame (it is a notification and gets no direct response); the session SHALL be marked cancelled so the in-flight `session/prompt` completes with `stopReason: "cancelled"` once the server's cancelled turn closes, instead of the normal `end_turn`. Unknown methods SHALL return a JSON-RPC method-not-found error; inbound frames with no `method` (an editor's response/error) SHALL be ignored, not answered.

#### Scenario: new session opens a server conversation rooted at the editor's directory

- **WHEN** the editor calls `session/new` with a working directory
- **THEN** a server conversation is opened whose working root is that directory (carried as the message origin), and an ACP session id is returned

#### Scenario: load resumes a conversation

- **WHEN** the editor calls `session/load` for a known session
- **THEN** the adapter resumes the mapped server conversation and replays its history as `session/update` notifications

#### Scenario: cancel stops the turn

- **WHEN** the editor sends `session/cancel` during a turn
- **THEN** the adapter forwards `CancelTurn` to the server, the in-flight server turn stops at its next checkpoint, and the pending `session/prompt` responds with `stopReason: "cancelled"`
