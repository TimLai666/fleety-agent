## MODIFIED Requirements

### Requirement: Interactive entry points share one terminal workspace

Bare `fleety` on a TTY SHALL open the terminal workspace at Chat; bare `fleety` on non-TTY SHALL print help and exit zero. `fleety chat`, legacy `fleety tui`, and bare TTY `fleety config` SHALL open the same workspace at Chat or Settings without duplicating connection or owner state. Settings and its nested Provider editor SHALL retain one continuous full-screen alternate-screen session during normal navigation and editing; the transition SHALL NOT expose the primary scrollback. A standalone Provider editor SHALL continue to initialize and restore its own terminal. A flow that explicitly requires a plain terminal, including OAuth browser interaction, SHALL suspend and restore the full-screen terminal as a paired exceptional transition.

#### Scenario: terminal detection selects a safe entry

- **WHEN** bare `fleety` runs with an interactive terminal
- **THEN** it SHALL open Chat with the current connection context
- **WHEN** bare `fleety` runs with piped stdin or captured stdout
- **THEN** it SHALL print help and SHALL NOT connect or consume input as a prompt

#### Scenario: Settings opens Add Provider without exposing scrollback

- **GIVEN** Settings is displaying the Providers & Models page in a full-screen alternate-screen terminal
- **WHEN** the user presses Enter and then presses `a`
- **THEN** the Add Provider type picker SHALL render in the same alternate-screen session
- **AND** the handoff SHALL NOT emit LeaveAlternateScreen followed by EnterAlternateScreen

#### Scenario: Provider preparation failure stays in Settings terminal

- **GIVEN** Settings is displaying the Providers & Models page
- **WHEN** the Provider connection, identity validation, or snapshot preparation fails
- **THEN** Settings SHALL remain on its existing full-screen terminal and display the failure state
- **AND** the primary scrollback SHALL NOT become visible

#### Scenario: standalone Provider editor owns its terminal

- **WHEN** the Provider editor is launched outside Settings
- **THEN** it SHALL initialize a full-screen terminal before drawing
- **AND** it SHALL restore the terminal when the editor exits

#### Scenario: OAuth uses an explicit paired plain-terminal transition

- **GIVEN** the Provider editor is embedded in Settings
- **WHEN** an OAuth action requires browser or plain-terminal interaction
- **THEN** the full-screen terminal SHALL be restored before the OAuth interaction
- **AND** a new full-screen terminal SHALL be initialized before interactive Provider or Settings drawing resumes
