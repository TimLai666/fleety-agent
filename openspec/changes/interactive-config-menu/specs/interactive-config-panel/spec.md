## ADDED Requirements

### Requirement: Bare `fleety config` opens a top-level menu with guided drill-down

On a TTY with no subcommand, `fleety config` SHALL open a top-level menu from which the user selects what to configure — at least Providers, Models, and Settings — and selecting an item SHALL enter that item's own screen. `Esc` SHALL return from a screen to the menu; a quit key SHALL exit. When not on a TTY, or when a subcommand is given, the existing non-interactive behavior SHALL be preserved.

#### Scenario: menu drill-down and back

- **WHEN** the user runs `fleety config` on a TTY, selects "Providers", then presses Esc
- **THEN** the Providers screen opens and Esc returns to the top-level menu (without exiting the program)

### Requirement: Guided provider and model editing

Adding a provider SHALL be guided: the user selects the provider type from a menu of the registered types, then is prompted for each required field in turn (name, and for an api type its base_url and api key) rather than entering one delimited line. Setting a model role SHALL be two-level: the user first selects a provider, then selects that provider's model. For an api provider with a base_url, the editor SHALL fetch the provider's model list from its `/models` endpoint and present it as a searchable, selectable list; if that fetch fails, returns nothing, or the provider has no queryable endpoint, the editor SHALL fall back to manual model-id entry without failing. All edits SHALL go through the same validation and atomic write as the non-interactive provider commands.

#### Scenario: model selection lists the chosen provider's models

- **WHEN** the user sets a model role, selects an api provider, and that provider's `/models` endpoint responds
- **THEN** the editor lists that provider's model ids for the user to search and pick, and does not mix in other providers' models

#### Scenario: model fetch failure degrades to manual entry

- **WHEN** the provider's `/models` endpoint cannot be reached or returns nothing (or the provider is an oauth type)
- **THEN** the editor lets the user type the model id instead, rather than erroring out

### Requirement: The key hints stay visible

The interactive config screens SHALL render the key hints (including how to go back and how to quit) on a line separate from the transient status/result message, so that performing an action and showing its result does not overwrite or hide the hints.

#### Scenario: hints survive an action's output

- **WHEN** the user performs an action that prints a result/status
- **THEN** the key hints (back / quit / navigation) remain visible rather than being overwritten by the result
