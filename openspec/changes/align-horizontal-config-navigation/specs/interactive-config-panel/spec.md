## ADDED Requirements

### Requirement: Horizontal choices follow their visual direction

Every interactive configuration choice control that renders its alternatives horizontally SHALL use Left to select the preceding item and Right to select the following item. The same controls SHALL continue to accept Up as an alias for Left and Down as an alias for Right. Their visible navigation hints SHALL identify Left and Right as the primary keys. Vertically rendered provider, model, and settings lists SHALL retain Up and Down navigation and SHALL NOT gain Left or Right aliases from this requirement.

#### Scenario: provider type follows horizontal direction

- **WHEN** the add-provider type choices are displayed on one horizontal line and the user presses Left or Right
- **THEN** the selection moves to the preceding or following displayed type respectively

#### Scenario: model role follows horizontal direction

- **WHEN** the model-role choices are displayed on one horizontal line and the user presses Left or Right
- **THEN** the selection moves to the preceding or following displayed role respectively

#### Scenario: OAuth action follows horizontal direction

- **WHEN** the Codex OAuth actions are displayed on one horizontal line and the user presses Left or Right
- **THEN** the selection moves to the preceding or following displayed action respectively

#### Scenario: unsaved-exit action follows horizontal direction

- **WHEN** the unsaved-exit choices are displayed on one horizontal line and the user presses Left or Right
- **THEN** the selection moves to the preceding or following displayed action respectively

#### Scenario: legacy directional keys remain compatible

- **WHEN** any horizontally rendered configuration choice has focus and the user presses Up or Down
- **THEN** Up performs the same bounded movement as Left and Down performs the same bounded movement as Right

#### Scenario: horizontal hints match the layout

- **WHEN** a horizontally rendered configuration choice is visible
- **THEN** its navigation hint presents Left and Right rather than Up and Down

#### Scenario: vertical lists keep vertical navigation

- **WHEN** a vertically rendered provider, model, or settings list has focus
- **THEN** Up and Down navigate the list and Left and Right do not change its selection under this requirement
