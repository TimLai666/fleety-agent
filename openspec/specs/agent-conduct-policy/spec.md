# agent-conduct-policy Specification

## Purpose

TBD - created by archiving change 'baseline-prompt-specs'. Update Purpose after archive.

## Requirements

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


<!-- @trace
source: default-auto-review-policy
updated: 2026-08-15
code:
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/main.rs
  - prompts/policy.md
  - crates/fleety-tools/src/config.rs
  - crates/fleety-tools/src/lib.rs
  - README.md
  - docs/env.md
  - docs/tools.md
-->

---
### Requirement: Auto review is unattended and fail-closed

When `auto_review` is active, the agent SHALL send the objective, bounded
context, sanitized candidate, risk class, and trusted danger signals to the cheap
reviewer. The candidate tool SHALL execute only after a valid exact approval. A
timeout, provider failure, invalid response, redaction failure, or protocol
violation SHALL deny the candidate and record a sanitized audit outcome.


<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->

---
### Requirement: Physical-presence actions require co-location

An action that needs physical presence at a place (operating local-only hardware, anything bound to a site) SHALL be performed only through a device that is actually located at that site, not routed to an arbitrary device.

#### Scenario: physical action routes to an on-site device

- **WHEN** a task needs physical presence at a site
- **THEN** the agent selects a device located at that site rather than a remote one


<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->

---
### Requirement: Intrusive UI control prefers the least-intrusive tier

When driving an interface, the agent SHALL prefer the least-intrusive capable tier in the order dedicated API/MCP, then browser CDP, then native computer-use, and SHALL fall to computer-use only when no higher tier can accomplish the task. Taking a screenshot to observe a screen SHALL be exempt from this ranking as a low-impact read.

#### Scenario: computer-use is the last resort

- **WHEN** a task can be done through a dedicated API/MCP or browser CDP
- **THEN** the agent uses that tier rather than native mouse/keyboard control
- **WHEN** only a screen observation is needed
- **THEN** a screenshot is allowed without escalating through the ranking


<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->

---
### Requirement: Unattended scheduled runs are mandate-bounded

A scheduled (unattended) run SHALL act only within the mandate and allowed-tools captured when the schedule was created, and SHALL NOT take destructive or outward-facing actions beyond that mandate without it being part of the schedule's intent.

#### Scenario: scheduled run honours its captured mandate

- **WHEN** a schedule fires
- **THEN** the run is constrained to the mandate and allowed_tools recorded at creation time


<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->

---
### Requirement: Mutating actions are audited and reversible

Every mutating action SHALL be recorded to the per-device audit log, and file mutations SHALL be backed up so they can be reverted via rollback.

#### Scenario: a mutation is auditable and reversible

- **WHEN** a file-mutating tool runs
- **THEN** the action is appended to the audit log and a backup is captured for rollback

<!-- @trace
source: baseline-prompt-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .agents/skills/spectra-archive/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .agents/skills/spectra-commit/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-audit.md
  - .opencode/skills/spectra-propose/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - CLAUDE.md
  - .opencode/commands/spectra-discuss.md
  - AGENTS.md
  - .opencode/commands/spectra-ask.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
-->