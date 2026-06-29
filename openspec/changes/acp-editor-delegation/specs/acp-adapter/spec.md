## ADDED Requirements

### Requirement: The adapter is a bidirectional, persistent ACP agent

The ACP adapter SHALL act as a bidirectional ACP agent — both answering the
editor's requests (session methods) and calling the editor's filesystem and
terminal methods — over a connection that persists for the session and services
routed tool calls while a turn is streaming. It SHALL advertise the editor-backed
tools it can service (gated by the editor's advertised capabilities) and SHALL
report the device (host) the editor session runs on. Conformant editors require no
changes.

#### Scenario: a routed tool call is serviced mid-turn

- **WHEN** the server routes an editor tool call to the adapter while a turn is streaming
- **THEN** the adapter translates it to the matching ACP editor request, awaits the editor's response, and returns the tool result without interrupting the stream

#### Scenario: only advertised editor capabilities are offered

- **WHEN** the adapter reads the editor's advertised capabilities at initialize
- **THEN** it advertises only the editor-backed tools the editor actually supports
