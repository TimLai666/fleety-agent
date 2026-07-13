## MODIFIED Requirements

### Requirement: Guided provider and model editing

Adding a provider SHALL be guided: the user selects the provider type from a menu of the registered types, then is prompted for each required field in turn (name, and for an api type its base_url and api key) rather than entering one delimited line. Setting a model role SHALL be two-level: the user first selects a provider, then selects that provider's model. For an api provider with a base_url, the editor SHALL fetch the provider's model list from its `/models` endpoint and present it as a searchable, selectable list. For an `oauth:codex` provider in the remote Server region, the editor SHALL request model IDs through the server's provider-model discovery operation when the connected server supports it. If discovery fails, returns nothing, the provider has no queryable endpoint, or the connected server lacks the discovery capability, the editor SHALL fall back to manual model-id entry without failing. An existing provider SHALL be editable in place: for an api provider the editor SHALL prompt to change its base_url and api key (its name fixed), and for an `oauth:codex` provider the editor SHALL offer that provider's OAuth actions ? sign in, sign out, and switch account (switch being sign out then sign in). Because the OAuth sign-in flow is asynchronous, opens a browser, and needs the plain terminal, the editor SHALL run those OAuth actions by saving the current config, leaving the full-screen editor, performing the sign-in or sign-out for the selected provider, and then reopening the editor ? never attempting the browser flow inside the full-screen UI. All edits SHALL go through the same validation and atomic write as the non-interactive provider commands.

#### Scenario: model selection lists the chosen API provider's models

- **WHEN** the user sets a model role, selects an api provider, and that provider's `/models` endpoint responds
- **THEN** the editor lists that provider's model ids for the user to search and pick, and does not mix in other providers' models

#### Scenario: model selection lists the chosen OAuth provider's models

- **WHEN** the user sets a model role, selects a signed-in `oauth:codex` provider in the remote Server region, and the server supports provider-model discovery
- **THEN** the editor lists the model ids returned for that provider and does not require a provider `base_url`

#### Scenario: model fetch failure degrades to manual entry

- **WHEN** the provider model endpoint or server discovery operation cannot be reached, returns nothing, returns an error, or the server lacks the required capability
- **THEN** the editor lets the user type the model id instead, and displays a fallback reason without crashing

#### Scenario: an existing API provider is editable

- **WHEN** the user selects an existing api provider and chooses edit
- **THEN** the editor prompts to change its base_url and api key and saves through the same validation and atomic write as the non-interactive commands

#### Scenario: an OAuth provider offers sign-in actions

- **WHEN** the user selects an existing `oauth:codex` provider and chooses edit
- **THEN** the editor offers sign in, sign out, and switch account for that provider

#### Scenario: an OAuth action leaves and re-enters the editor

- **WHEN** the user chooses sign in, sign out, or switch for an `oauth:codex` provider
- **THEN** the editor saves the config, leaves the full-screen UI, runs that provider's sign-in or sign-out, and then reopens the editor with the result shown; the browser flow never runs inside the full-screen UI

