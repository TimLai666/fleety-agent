## ADDED Requirements

### Requirement: Native desktop control on any device

The system SHALL provide native `computer_screenshot`, `computer_move`, `computer_click`, `computer_type`, `computer_key`, and `computer_scroll` tools, registered on the server and every device daemon so `device_exec` can drive a specific device's desktop. `computer_screenshot` SHALL be read-risk; the input tools SHALL be mutate-risk. On a host without a usable display session the tools SHALL return an actionable error rather than crashing.

#### Scenario: headless host returns an actionable error

- **WHEN** an input tool is invoked on a host with no display session
- **THEN** it returns an actionable error stating a real display is required

#### Scenario: key with modifiers always releases modifiers

- **WHEN** `computer_key` presses a key with modifiers and the key press errors
- **THEN** the held modifiers are still released before the error is returned
