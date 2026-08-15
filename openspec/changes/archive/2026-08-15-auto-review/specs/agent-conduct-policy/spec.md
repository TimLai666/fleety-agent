## MODIFIED Requirements

### Requirement: Full access by default

The agent SHALL operate with full access by default: mutate tools run without per-call approval. Per-call interactive approval SHALL apply only when the runtime policy is `require_approval`. When the runtime policy is `auto_review`, every mutate and critical tool SHALL be reviewed by the unattended cheap-model gate before execution, while read tools SHALL remain direct.

#### Scenario: default posture runs mutations directly

- **WHEN** the runtime policy is the default and a mutate tool is invoked
- **THEN** the action runs without a per-call approval prompt and is recorded in the audit log

#### Scenario: auto posture reviews a mutation

- **WHEN** the runtime policy is `auto_review` and a mutate tool is invoked
- **THEN** the action waits for a cheap-model decision and no human prompt is required

#### Scenario: auto posture reviews a critical action

- **WHEN** the runtime policy is `auto_review` and a critical tool is invoked
- **THEN** the action is submitted to the cheap-model decision instead of being rejected solely for its risk class
