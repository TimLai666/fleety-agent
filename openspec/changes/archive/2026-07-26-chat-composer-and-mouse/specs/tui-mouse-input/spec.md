## ADDED Requirements

### Requirement: Mouse reporting is scoped to the Chat workspace

Fleety SHALL enable terminal mouse reporting when the Chat workspace loop starts and SHALL disable it on every exit from that loop, including exits that hand control to another route and exits caused by a panic. When mouse reporting cannot be enabled, Chat SHALL continue with keyboard input only.

#### Scenario: reporting is released when Chat hands off to Settings

- **WHEN** the user navigates from Chat to Settings and the Chat loop exits
- **THEN** mouse reporting SHALL be disabled before the Settings panel draws

#### Scenario: a panic does not leave the terminal reporting mouse events

- **GIVEN** mouse reporting was enabled for Chat
- **WHEN** the process panics
- **THEN** the disable sequence SHALL be emitted before the terminal is restored

#### Scenario: enabling fails and Chat still runs

- **GIVEN** the terminal rejects the enable sequence
- **WHEN** Chat starts
- **THEN** Chat SHALL run with keyboard input only and SHALL NOT emit a disable sequence on exit

### Requirement: Only Chat receives mouse events

The terminal event source SHALL carry keyboard and mouse events on one ordered stream. Its default read SHALL return keyboard events only, and mouse events SHALL be reachable through a separate read used by Chat. Routes that do not handle mouse input SHALL require no change to remain correct.

#### Scenario: Settings is unaffected by mouse activity

- **GIVEN** the active route is Settings
- **WHEN** the user moves and clicks the mouse
- **THEN** Settings SHALL observe no input and SHALL NOT change state

#### Scenario: ordering between a click and a keystroke is preserved

- **GIVEN** the user clicks and then immediately types
- **WHEN** Chat reads the event stream
- **THEN** the click SHALL be delivered before the keystroke

### Requirement: Mouse events are resolved against the last rendered geometry

Chat SHALL record the drawn area of the transcript and of the composer on each frame and SHALL resolve a mouse event's target from those recorded areas. A mouse event that arrives before any frame has been drawn SHALL be ignored.

#### Scenario: a click lands where the user saw the composer

- **GIVEN** Chat has drawn a frame
- **WHEN** the user clicks inside the composer's drawn area
- **THEN** the event SHALL be routed to the composer

#### Scenario: no frame has been drawn yet

- **WHEN** a mouse event arrives before the first frame
- **THEN** the event SHALL be ignored and SHALL NOT change any state

### Requirement: The wheel scrolls the transcript only over the transcript

A wheel event over the transcript SHALL scroll the conversation. A wheel event outside the transcript SHALL NOT scroll the conversation.

#### Scenario: wheel over the transcript scrolls back and returns

- **GIVEN** the transcript has more content than fits on screen and is following the newest output
- **WHEN** the user scrolls the wheel up once over the transcript, then down once
- **THEN** the transcript SHALL scroll back and then return to the newest output

#### Scenario: wheel over the composer leaves the transcript alone

- **GIVEN** the transcript is following the newest output
- **WHEN** the user scrolls the wheel over the composer
- **THEN** the transcript scroll position SHALL be unchanged

### Requirement: Pointer gestures in the composer place the caret and select text

A press inside the composer SHALL move the caret to the pressed position. A drag begun inside the composer SHALL extend a selection and SHALL continue to be delivered to the composer after the pointer leaves the composer's area, until the button is released. On release of a selection, Fleety SHALL place the selected text on the system clipboard and SHALL report the outcome in the status line, including when the clipboard is unavailable.

#### Scenario: clicking moves the insertion point

- **GIVEN** the composer contains `hello world` with the caret at the end
- **WHEN** the user clicks three columns into the text and types `X`
- **THEN** the composer SHALL contain `helXlo world`

#### Scenario: a released drag copies the selection

- **GIVEN** the composer contains `hello world`
- **WHEN** the user presses at the first column, drags five columns right, and releases
- **THEN** the selected text `hello` SHALL be placed on the system clipboard and the status line SHALL report the number of characters copied

#### Scenario: the clipboard refuses the write

- **GIVEN** the system clipboard is unavailable
- **WHEN** a selection is released
- **THEN** the status line SHALL state that nothing was copied and Chat SHALL continue running

#### Scenario: a drag that leaves the composer keeps selecting

- **GIVEN** a drag was begun inside the composer
- **WHEN** the pointer moves outside the composer's area and the button is released there
- **THEN** the gesture SHALL still be delivered to the composer and the selection SHALL be completed

### Requirement: Transcript text selection belongs to the terminal

Fleety SHALL NOT implement text selection for the transcript. Fleety SHALL document that transcript text is selected with the terminal's own modifier-held drag while mouse reporting is active.

#### Scenario: user documentation states the transcript selection gesture

- **WHEN** a user reads the CLI documentation for the Chat TUI
- **THEN** it SHALL state that holding Shift while dragging selects transcript text with the terminal
