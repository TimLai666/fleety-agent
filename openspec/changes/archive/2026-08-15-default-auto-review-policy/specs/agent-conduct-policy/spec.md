## RENAMED Requirements

### Requirement: Default policy uses auto review

FROM: Full access by default
TO: Default policy uses auto review

## MODIFIED Requirements

### Requirement: Default policy uses auto review

The agent SHALL use the unattended `auto_review` posture by default: read tools run directly, while mutate and critical tools pass through the cheap-model review before execution. Explicit `full_access` SHALL remain available for operators who deliberately choose direct audited execution, and explicit `require_approval` SHALL remain available for interactive approval. When the runtime policy is `auto_review`, every mutate and critical tool SHALL be reviewed by the unattended cheap-model gate while read tools SHALL remain direct.

#### Scenario: default posture reviews mutations

- **WHEN** the runtime policy is unset and a mutate tool is invoked
- **THEN** the action waits for a cheap-model decision and no human prompt is required

##### Example: default write uses auto review

- **GIVEN** `FLEETY_POLICY` is unset and the agent calls `write_file` to update `notes.txt`
- **WHEN** the call reaches the shared execution gate
- **THEN** the read/write distinction selects `auto_review`, the cheap reviewer evaluates the mutation, and no human approval request is emitted

#### Scenario: explicit full access runs mutations directly

- **WHEN** the runtime policy is explicitly `full_access` and a mutate tool is invoked
- **THEN** the action runs without a per-call approval prompt and is recorded in the audit log

#### Scenario: explicit require approval pauses mutations

- **WHEN** the runtime policy is explicitly `require_approval` and a mutate tool is invoked
- **THEN** the action waits for the interactive approval flow before execution

#### Scenario: default posture reviews critical actions without human participation

- **WHEN** the runtime policy is unset and a critical tool is invoked
- **THEN** the action is submitted to the cheap-model decision instead of being rejected solely for its risk class or waiting for a human

##### Example: default critical command uses the reviewer

- **GIVEN** `FLEETY_POLICY` is unset and the candidate is `run_command` with `rm -rf /`
- **WHEN** the critical-command detector produces its trusted danger signal
- **THEN** the candidate is sent to the cheap reviewer with no human approval request, and only an exact reviewer approval can execute it
