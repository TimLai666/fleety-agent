## ADDED Requirements

### Requirement: Composer wraps long lines instead of scrolling sideways

The chat composer SHALL wrap a line that exceeds the box width onto further visual rows and SHALL grow the box height with the wrapped row count up to the existing maximum, after which it SHALL scroll vertically. The composer SHALL NOT scroll horizontally to keep the caret visible.

#### Scenario: a long line stays readable from its start

- **GIVEN** the composer is 28 columns wide and empty
- **WHEN** the user types `the quick brown fox jumps over the lazy dog`
- **THEN** both the start and the end of the typed text SHALL be visible, on different rows

### Requirement: Composer supports undo, kill and yank

The chat composer SHALL maintain an edit history and SHALL support undo and redo. It SHALL support deleting the previous word, deleting to the end of the line, and deleting to the start of the line, and SHALL support re-inserting the most recent such deletion at the caret.

#### Scenario: a word deletion is undone

- **GIVEN** the composer contains `hello world`
- **WHEN** the user deletes the previous word and then requests undo
- **THEN** the composer SHALL contain `hello world` again

##### Example: word kill then undo

| Step | Composer contents |
| ---- | ----------------- |
| after typing | `hello world` |
| after delete-previous-word | `hello ` |
| after undo | `hello world` |

### Requirement: Fleety claims only its own chords and delegates the rest

The chat TUI SHALL intercept only the key chords that carry a Fleety-specific meaning and SHALL pass every other key to the composer's own key map, so that the two key maps cannot diverge. Where a chord means one thing to Fleety and another to the composer, the Fleety meaning SHALL win and the composer SHALL NOT also act on it.

#### Scenario: the attachment-clearing chord does not cut the draft

- **GIVEN** the composer contains `keep me` and one attachment is staged
- **WHEN** the user presses the chord that clears staged attachments
- **THEN** the staged attachments SHALL be cleared and the composer SHALL still contain `keep me`

#### Scenario: an editing chord Fleety does not claim reaches the composer

- **GIVEN** the composer contains `hello world`
- **WHEN** the user presses the chord for deleting the previous word
- **THEN** the composer SHALL contain `hello `

### Requirement: Prefilled composer content places the caret at the end

When the chat TUI replaces the composer's contents programmatically — restoring a draft that was not accepted, or seeding content — the caret SHALL be placed at the end of the inserted text so that the next keystroke continues the text.

#### Scenario: a rejected attach command is restored ready to edit

- **GIVEN** the user submitted an attach command whose path does not resolve
- **WHEN** the command text is restored into the composer and the user types a character
- **THEN** the character SHALL appear at the end of the restored text

### Requirement: Composer state survives independently of its implementation

The composer's unsent text, caret position within that text, and staged attachments SHALL survive navigation between routes and recoverable reconnection. The caret's position SHALL be preserved as a position within the text, independent of how the text is laid out on screen.

#### Scenario: caret position survives a reconnect

- **GIVEN** the composer contains a multi-line draft and the caret is not at the end
- **WHEN** Chat reconnects to a different profile and the draft is retained
- **THEN** the composer text SHALL be unchanged and the caret SHALL occupy the same position within that text
