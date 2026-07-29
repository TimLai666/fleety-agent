## ADDED Requirements

### Requirement: Interactive entry points share one terminal workspace

Bare `fleety` on a TTY SHALL open the terminal workspace at Chat; bare `fleety` on non-TTY SHALL print help and exit zero. `fleety chat`, legacy `fleety tui`, and bare TTY `fleety config` SHALL open the same workspace at Chat or Settings without duplicating connection or owner state.

#### Scenario: terminal detection selects a safe entry

- **WHEN** bare `fleety` runs with an interactive terminal
- **THEN** it SHALL open Chat with the current connection context
- **WHEN** bare `fleety` runs with piped stdin or captured stdout
- **THEN** it SHALL print help and SHALL NOT connect or consume input as a prompt

### Requirement: The workspace keeps context and navigation visible

The workspace SHALL display the selected profile, connection state, active provider/model, and current route in a persistent header. It SHALL provide Chat, Conversations, Settings, profile picker, contextual help, and command palette routes with a footer generated from the active route and modal state.

#### Scenario: reconnect state remains understandable

- **WHEN** the active Server connection drops and automatic reconnection begins
- **THEN** the header SHALL show Reconnecting with attempt/backoff information while the current input and route remain available

##### Example: editable draft during backoff

- **GIVEN** Chat contains draft `deploy` when the link drops
- **WHEN** reconnect attempt 3 waits 2000 ms and the user types ` now` or opens Help
- **THEN** the header shows `Reconnecting 3 (2000 ms)`, the draft becomes `deploy now`, and Help can open without cancelling recovery

### Requirement: Workspace key behavior is state-consistent

Esc SHALL close the active modal or navigate back one route; `?` SHALL open contextual help; Ctrl+K SHALL open the command palette. Ctrl+C SHALL request cancellation while a turn is active and SHALL only exit while idle or after explicit confirmation when unsent or dirty state exists.

#### Scenario: Esc does not discard hidden state

- **GIVEN** a Settings editor has an unsaved value
- **WHEN** the user presses Esc
- **THEN** the editor SHALL close or ask for a dirty-state decision and SHALL NOT silently discard the value or exit the workspace

### Requirement: Workspace rendering is responsive and Unicode-safe

Supported terminal sizes SHALL render header, active content, status/error details, and contextual keys without overlap. Below the minimum size, the workspace SHALL render a stable resize message and retain quit/help handling. Rendering SHALL account for CJK and emoji display width and SHALL NOT emit replacement glyphs.

#### Scenario: size and text matrix renders safely

- **WHEN** the workspace renders at 120×30, 80×24, 50×16, and a below-minimum size with ASCII, CJK, emoji, and long endpoints
- **THEN** supported sizes SHALL keep essential context and actions visible, and the below-minimum size SHALL show only resize guidance without panic

### Requirement: Notices preserve actionable errors

Errors SHALL be represented as notices with severity, summary, optional details, remediation, and persistence policy. A new transient status SHALL NOT overwrite an unresolved mutation error or conflict; the user SHALL be able to inspect details, retry, or dismiss it.

#### Scenario: catalog failure survives navigation

- **WHEN** model discovery fails and the user opens then closes contextual help
- **THEN** the model error and Retry/manual-entry remediation SHALL remain available until retried or dismissed

##### Example: unresolved catalog notice

- **GIVEN** catalog loading failed with `backend unavailable`
- **WHEN** the user opens Help, returns, and a transient Connected status arrives
- **THEN** the original details plus Retry and Enter model ID actions remain the visible unresolved notice
