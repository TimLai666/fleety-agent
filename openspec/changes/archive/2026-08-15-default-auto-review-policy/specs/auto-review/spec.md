## MODIFIED Requirements

### Requirement: Auto review gates unattended tool execution

The runtime SHALL provide `auto_review` as the default execution policy when `FLEETY_POLICY` is unset, empty, or unrecognized. When `auto_review` is active, read tools SHALL execute directly, while every mutate and critical tool call SHALL be submitted to the auto reviewer before execution. Explicit `full_access` and `require_approval` values SHALL retain their existing execution semantics. The policy SHALL apply to workspace tools, device-routed tools, remote execution, subagents, scheduled turns, WebSocket turns, SSE turns, and recovery continuations.

#### Scenario: a fresh runtime uses auto review

- **WHEN** the runtime starts without an explicit policy
- **THEN** it selects `auto_review` and applies the unattended reviewer to every mutate and critical tool call

##### Example: unset policy is not full access

- **GIVEN** `FLEETY_POLICY` is unset and the server has no stored policy
- **WHEN** a `run_command` candidate is produced
- **THEN** the candidate is sent to the cheap reviewer before execution and no human approval frame is emitted

#### Scenario: an explicit direct policy remains an override

- **WHEN** `FLEETY_POLICY=full_access`
- **THEN** mutate tools retain direct audited execution and critical tools retain their existing deterministic guard behavior

#### Scenario: an explicit interactive policy remains an override

- **WHEN** `FLEETY_POLICY=require_approval`
- **THEN** non-read tools retain the interactive approval flow
