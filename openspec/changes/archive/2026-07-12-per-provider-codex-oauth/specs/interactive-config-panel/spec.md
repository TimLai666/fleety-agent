## MODIFIED Requirements

### Requirement: Guided provider and model editing

Adding a provider SHALL be guided: the user selects the provider type from a menu of the registered types, then is prompted for each required field in turn (name, and for an api type its base_url and api key) rather than entering one delimited line. Setting a model role SHALL be two-level: the user first selects a provider, then selects that provider's model. For an api provider with a base_url, the editor SHALL fetch the provider's model list from its `/models` endpoint and present it as a searchable, selectable list; if that fetch fails, returns nothing, or the provider has no queryable endpoint, the editor SHALL fall back to manual model-id entry without failing. An **existing** provider SHALL be editable in place: for an api provider the editor SHALL prompt to change its base_url and api key (its name fixed), and for an `oauth:codex` provider the editor SHALL offer that provider's OAuth actions — sign in, sign out, and switch account (switch being sign out then sign in). Because the OAuth sign-in flow is asynchronous, opens a browser, and needs the plain terminal, the editor SHALL run those OAuth actions by saving the current config, leaving the full-screen editor, performing the sign-in or sign-out for the selected provider, and then reopening the editor — never attempting the browser flow inside the full-screen UI. All edits SHALL go through the same validation and atomic write as the non-interactive provider commands.

#### Scenario: model selection lists the chosen provider's models

- **WHEN** the user sets a model role, selects an api provider, and that provider's `/models` endpoint responds
- **THEN** the editor lists that provider's model ids for the user to search and pick, and does not mix in other providers' models

#### Scenario: model fetch failure degrades to manual entry

- **WHEN** the provider's `/models` endpoint cannot be reached or returns nothing (or the provider is an oauth type)
- **THEN** the editor lets the user type the model id instead, rather than erroring out

#### Scenario: an existing api provider is editable

- **WHEN** the user selects an existing api provider and chooses edit
- **THEN** the editor prompts to change its base_url and api key and saves through the same validation and atomic write as the non-interactive commands

#### Scenario: an oauth provider offers sign-in actions

- **WHEN** the user selects an existing `oauth:codex` provider and chooses edit
- **THEN** the editor offers sign in, sign out, and switch account for that provider

#### Scenario: an oauth action leaves and re-enters the editor

- **WHEN** the user chooses sign in (or sign out, or switch) for an `oauth:codex` provider
- **THEN** the editor saves the config, leaves the full-screen UI, runs that provider's sign-in or sign-out, and then reopens the editor with the result shown — the browser flow never runs inside the full-screen UI

### Requirement: The key hints stay visible

Every interactive config screen — the top-level menu, the three-region panel, and the provider editor (browse, the add-provider wizard, the set-model wizard, the timezone picker, and the edit and OAuth-action flows) — SHALL render the key hints (including how to go back and how to quit) on a line separate from the transient status/result message, so that performing an action and showing its result does not overwrite or hide the hints.

#### Scenario: hints survive an action's output

- **WHEN** the user performs an action that prints a result/status (for example adding a provider, which prints "added provider 'X'")
- **THEN** the key hints (back / quit / navigation) remain visible on their own line rather than being overwritten by the result

#### Scenario: every config screen keeps its hints

- **WHEN** any of the config screens (menu, three-region panel, provider editor and its wizards, timezone picker) is showing
- **THEN** its key hints are on a line that action or status output cannot overwrite
