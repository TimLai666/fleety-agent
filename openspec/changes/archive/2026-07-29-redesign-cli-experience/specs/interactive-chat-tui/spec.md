## ADDED Requirements

### Requirement: Chat participates in the shared workspace context

Chat SHALL render within the terminal workspace and SHALL use the shared selected profile, connection state, provider/model, notices, and navigation. Chat reconnect and conversation resume SHALL update shared context rather than maintaining a separate hidden connection identity.

#### Scenario: profile context matches the active chat transport

- **WHEN** the workspace reconnects Chat after a profile switch
- **THEN** the header profile, Server identity, model, and Chat transport SHALL all come from the new connection before message submission is enabled

##### Example: stale A transport cannot submit as B

- **GIVEN** the header selected profile is `B` but the retained transport context still identifies profile `A`
- **WHEN** the user presses Enter
- **THEN** the draft remains unchanged and no UserMessage is sent until a `B` Welcome and model snapshot atomically replace the context

### Requirement: Chat input survives route changes and recoverable connection loss

Unsent text and attachments SHALL survive navigation to Conversations, Settings, contextual help, and recoverable reconnect states. They SHALL be discarded only after explicit confirmation or forced process termination.

#### Scenario: inspect settings without losing a draft

- **GIVEN** Chat contains unsent multi-line text and an attachment
- **WHEN** the user opens Settings and returns to Chat
- **THEN** the text, cursor position, and attachment SHALL be unchanged
