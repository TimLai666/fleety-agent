## ADDED Requirements

### Requirement: Chat draws into an inline viewport and leaves the screen intact

Chat SHALL render into a viewport at the bottom of the terminal rather than an alternate screen. Starting Chat SHALL NOT clear or replace what the terminal was already showing, and leaving Chat SHALL leave the conversation on screen.

#### Scenario: prior terminal output survives a Chat session

- **GIVEN** the terminal shows the output of a previous command
- **WHEN** the user starts Chat, exchanges a message, and leaves
- **THEN** the previous output SHALL still be above the conversation, and the conversation SHALL still be on screen

#### Scenario: other panels are unaffected

- **WHEN** the user opens Settings from Chat
- **THEN** Settings SHALL keep using a full-screen alternate-screen terminal of its own

### Requirement: Completed conversation content becomes terminal history

A message that is complete SHALL be written into the terminal's own scrollback as styled text, and SHALL NOT be redrawn by Fleety afterwards. Once written, its scrolling and its text selection SHALL be the terminal's responsibility.

#### Scenario: a finished exchange leaves the viewport

- **GIVEN** the user sends a message and the assistant reply completes
- **WHEN** the next frame is drawn
- **THEN** both the message and the reply SHALL be in the terminal's scrollback, and the viewport SHALL contain only the composer and the status line

#### Scenario: history outlives the process

- **GIVEN** a conversation with several exchanges
- **WHEN** the user quits Chat
- **THEN** the exchanges SHALL remain visible in the terminal

### Requirement: A streaming reply is visible before it completes

While an assistant reply is streaming, the portion whose markdown blocks have closed SHALL be written to scrollback as it settles, and the portion still being written SHALL be drawn in the viewport and updated as new content arrives. Content SHALL NOT be written to scrollback while the markdown block it belongs to is still open.

#### Scenario: settled paragraphs move to history mid-reply

- **GIVEN** an assistant reply is streaming and its first paragraph has ended
- **WHEN** further content arrives
- **THEN** the first paragraph SHALL be in scrollback and SHALL NOT be redrawn, and the newer content SHALL be in the viewport

#### Scenario: an unclosed code fence stays in the viewport

- **GIVEN** a streaming reply has opened a fenced code block that has not closed
- **WHEN** a frame is drawn
- **THEN** the unclosed block SHALL remain in the viewport and SHALL NOT be written to scrollback

#### Scenario: the reply completes

- **WHEN** the assistant reply finishes
- **THEN** any remaining content SHALL be written to scrollback and the viewport SHALL return to the composer and the status line

### Requirement: The viewport is bounded

The viewport SHALL be sized to the content still being written plus the composer and the status line, and SHALL NOT exceed half the terminal height. When the content exceeds that bound, the viewport SHALL scroll within itself and SHALL show the newest content.

#### Scenario: a long unsettled block does not take over the screen

- **GIVEN** a streaming reply contains an open code block longer than half the terminal height
- **WHEN** a frame is drawn
- **THEN** the viewport SHALL occupy at most half the terminal height and SHALL show the end of the block

### Requirement: Resizing preserves the history

When the terminal size changes, Fleety SHALL re-emit the conversation history it has written rather than relying on the terminal's own reflow.

#### Scenario: narrowing the terminal does not corrupt earlier output

- **GIVEN** a conversation with several exchanges in scrollback
- **WHEN** the terminal is made narrower and a frame is drawn
- **THEN** the history SHALL be re-emitted at the new width with the same content and no leftover fragments

### Requirement: Fleety does not take the mouse

Fleety SHALL NOT enable terminal mouse reporting, and its input pipeline SHALL carry keyboard events only. Scrolling the conversation and selecting its text SHALL be performed with the terminal's own mouse handling.

#### Scenario: dragging selects with the terminal

- **GIVEN** Chat is running
- **WHEN** the user drags across the conversation without holding any modifier
- **THEN** the terminal SHALL perform its own text selection and Fleety SHALL receive no event

#### Scenario: the wheel scrolls the terminal's history

- **WHEN** the user scrolls the wheel
- **THEN** the terminal SHALL scroll its own scrollback and Fleety SHALL NOT change what it draws
