## ADDED Requirements

### Requirement: Interactive screen edits an existing provider in place

The interactive `config provider edit` screen SHALL let the operator edit the fields of an already-defined provider (base URL, model, key) without removing and re-adding it, reusing the same field-update semantics as the `config provider set` subcommand so that only the fields the operator changes are updated and unchanged fields are preserved. A provider that a group or role references SHALL be editable without first unbinding it. The edited configuration SHALL be validated and written through the shared atomic writer on save.

#### Scenario: editing a referenced provider's model keeps its bindings

- **WHEN** a provider that a group or role references is selected, its model is changed in the interactive screen, and the screen is saved
- **THEN** the provider retains its group and role bindings, only the model field changes, and the file is written through validation

#### Scenario: unchanged fields are preserved on edit

- **WHEN** a provider's key is edited while its base URL and model are left untouched
- **THEN** the saved provider keeps its original base URL and model and only the key changes

### Requirement: Interactive screen removes groups and unsets roles

The interactive `config provider edit` screen SHALL support removing a group and unsetting a role, matching the `config group remove` and `config role unset` subcommands. Removing a group that a role still targets SHALL be rejected with a message naming the referring role and SHALL leave the group in place. Unsetting a role that is not defined SHALL be reported without changing the configuration, and unsetting a defined role SHALL remove its binding.

#### Scenario: removing a referenced group is blocked

- **WHEN** a group removal is requested in the interactive screen while a role still targets that group
- **THEN** the removal is rejected with a message naming the referring role and the group remains

#### Scenario: unsetting a role removes its binding

- **WHEN** a defined role is unset in the interactive screen and the screen is saved
- **THEN** the role no longer appears and the configuration is written through validation

### Requirement: Interactive screen validates provider fields per field

The interactive `config provider edit` screen SHALL collect a new or edited provider's fields through per-field prompts rather than a single comma-separated line. An empty required field (name, base URL, or model) SHALL be rejected with a message identifying the offending field, and the provider SHALL NOT be added or updated until every required field is non-empty.

#### Scenario: an empty required field is rejected by name

- **WHEN** the per-field provider entry is completed with an empty model field
- **THEN** the entry is rejected with a message naming the model field and no provider is added or updated

#### Scenario: a completed per-field entry adds the provider

- **WHEN** name, base URL, and model are all supplied through the per-field prompts and the entry is confirmed
- **THEN** the provider is added with those field values

### Requirement: Interactive screen confirms provider deletion

The interactive `config provider edit` screen SHALL require an explicit confirmation before deleting a provider, so that a single keypress cannot remove a provider. The confirmation prompt SHALL name the provider being deleted. Accepting the confirmation SHALL remove the provider (subject to the existing group/role reference guard); cancelling SHALL leave the configuration unchanged.

#### Scenario: delete asks for confirmation

- **WHEN** the delete key is pressed on a selected provider
- **THEN** the screen enters a confirmation prompt naming the provider and no provider is removed until the confirmation is accepted

#### Scenario: cancelling delete keeps the provider

- **WHEN** the delete confirmation is cancelled
- **THEN** the selected provider remains in the configuration unchanged