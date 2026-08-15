## MODIFIED Requirements

### Requirement: List recent audit entries

The system SHALL provide a `history_list` tool that returns recent audit-log entries (tool calls, results, replies, and auto-review decisions) for the current device, with an optional `limit` (default 20). Every tool call and every auto-review decision SHALL be recorded to the per-device audit log so any mutating action and its authorization outcome can be explained after the fact. Auto-review entries SHALL include the decision, risk class, tool, the canonical resolved reviewer provider/model label, danger-signal codes, latency, and sanitized reason, and SHALL omit raw arguments, prompt text, tokens, API keys, and passwords. The label SHALL describe the provider role actually selected: a cheap selector that aliases to the main provider SHALL be recorded as `main`, while a distinct configured cheap provider SHALL be recorded as `cheap`.

#### Scenario: limit the returned entries

- **WHEN** `history_list` is called with `limit=5` and the log has more than 5 entries
- **THEN** at most the 5 most recent entries are returned

#### Scenario: audit contains an auto-review decision without secrets

- **WHEN** an auto-reviewed candidate is approved or denied
- **THEN** `history_list` contains its sanitized decision metadata and contains no raw secret or candidate argument

#### Scenario: audit identifies a main fallback reviewer

- **WHEN** auto review requests the `cheap` tier and no distinct cheap provider is configured
- **THEN** the audit entry records `provider_model` as `main`

##### Example: cheap selector aliases main

- **GIVEN** the runtime has a `main` provider and no `cheap` provider
- **WHEN** the auto-review gate evaluates a mutate or critical tool call
- **THEN** the recorded `provider_model` value is `main`

#### Scenario: audit identifies an explicitly configured cheap reviewer

- **WHEN** auto review requests the `cheap` tier and the runtime has a distinct configured cheap provider
- **THEN** the audit entry records `provider_model` as `cheap`

<!-- @trace
source: fix-audit-provider-model-label
updated: 2026-08-15
code:
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-server/src/storage.rs
-->
