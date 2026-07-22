## ADDED Requirements

### Requirement: Provider, authentication, model catalog, and roles form one workflow

The canonical Provider surface SHALL show each Provider's type, endpoint class, authentication state, catalog state, and bound roles. OAuth login, logout, and status SHALL be actions on a named Provider; model selection SHALL proceed through Provider selection, catalog load, model selection or manual ID, and role confirmation.

#### Scenario: OAuth provider status is visible before model selection

- **WHEN** an OAuth Provider is not signed in
- **THEN** the Provider surface SHALL show Not signed in, offer Login, and prevent a catalog request from being represented as an anonymous endpoint failure

##### Example: Codex OAuth before catalog

- **GIVEN** Provider `tingzhen-codex` has type `oauth:codex` and no stored credential
- **WHEN** the user starts main-model selection
- **THEN** the row shows Not signed in and Login, and no model-catalog request is sent until authentication succeeds

### Requirement: Model discovery failure has retry and manual recovery

Catalog loading SHALL expose Loading, Available, Failed, and Unavailable states. A failure SHALL retain the backend error details and offer Retry and Enter model ID without losing the selected Provider or role.

#### Scenario: retry preserves selection

- **WHEN** catalog loading fails for Provider `tingzhen-codex`, role `main`, and the user selects Retry
- **THEN** the next request SHALL use the same connected Server, Provider, and role, while the previous error remains inspectable until the retry completes

### Requirement: Provider commands and TUI share one application service

Canonical provider/model commands, compatibility aliases, and the TUI SHALL use the same validation, owner routing, OAuth status, catalog fetch, role binding, and error mapping service.

#### Scenario: invalid provider input agrees across surfaces

- **WHEN** the same invalid Provider name or endpoint is submitted through command mode and the TUI
- **THEN** both surfaces SHALL reject it before mutation with the same error kind and remediation

##### Example: unsafe Provider name

- **GIVEN** the Provider name is `../outside`
- **WHEN** it is submitted through `fleety provider add` and the TUI add wizard
- **THEN** both return the same validation kind and safe-name remediation, and the Server records no mutation
