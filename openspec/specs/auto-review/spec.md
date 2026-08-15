# auto-review Specification

## Purpose

TBD - created by archiving change 'auto-review'. Update Purpose after archive.

## Requirements

### Requirement: Auto review gates unattended tool execution

The runtime SHALL provide an opt-in `auto_review` policy. When `auto_review` is active, read tools SHALL execute directly, while every mutate and critical tool call SHALL be submitted to the auto reviewer before execution. The policy SHALL apply to workspace tools, device-routed tools, remote execution, subagents, scheduled turns, WebSocket turns, SSE turns, and recovery continuations.

#### Scenario: a read tool bypasses auto review

- **WHEN** `auto_review` is active and the agent calls a read tool
- **THEN** the read tool executes without an auto-review model call

#### Scenario: a mutate tool requires auto review

- **WHEN** `auto_review` is active and the agent calls a mutate tool
- **THEN** the candidate is not executed until the cheap reviewer returns a valid approval

#### Scenario: a critical tool requires auto review

- **WHEN** `auto_review` is active and the agent calls a critical tool
- **THEN** the candidate is not rejected solely because it is critical and is submitted to the cheap reviewer before execution


<!-- @trace
source: auto-review
updated: 2026-08-15
code:
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/agent-core/src/agent.rs
  - docs/tools.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/event.rs
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: Auto review receives objective and trusted danger signals

The auto reviewer SHALL receive a bounded, redacted review context containing the current user objective, relevant user and assistant context, candidate tool name, candidate arguments, risk class, and deterministic danger signals. Danger signals SHALL be machine-generated evidence that is clearly separated from candidate content and SHALL identify irreversible command patterns or sensitive-path targets without exposing secrets.

#### Scenario: reviewer sees why a dangerous command was detected

- **WHEN** the candidate command matches a raw-disk-write detector
- **THEN** the review context includes a warning that identifies the raw-disk-write danger category and instructs the reviewer to approve it only when the stated objective clearly requires it

#### Scenario: candidate text cannot rewrite review policy

- **WHEN** a tool argument or user-provided filename contains instructions directed at the reviewer
- **THEN** the reviewer receives that content as untrusted candidate data and SHALL continue to follow the trusted review instructions


<!-- @trace
source: auto-review
updated: 2026-08-15
code:
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/agent-core/src/agent.rs
  - docs/tools.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/event.rs
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: Auto review uses a strict fail-closed decision

The cheap reviewer SHALL return exactly one structured decision of `approve` or `deny` with a bounded reason. A timeout, provider failure, exhausted retry budget, missing context, invalid JSON, unsupported decision, tool-call output, or oversized response SHALL produce `deny` and SHALL NOT execute the candidate tool.

#### Scenario: reviewer approves a justified critical action

- **WHEN** the cheap reviewer returns `{"decision":"approve","reason":"the requested disk operation is required by the stated maintenance objective"}`
- **THEN** the candidate critical tool executes and the approval is recorded

#### Scenario: reviewer denies an unjustified critical action

- **WHEN** the cheap reviewer returns `{"decision":"deny","reason":"the destructive target is unrelated to the user's objective"}`
- **THEN** the candidate tool does not execute and the agent receives a synthetic denial result

#### Scenario: unavailable reviewer denies automatically

- **WHEN** the cheap provider times out or returns an invalid response
- **THEN** the candidate tool does not execute, no human approval is requested, and the failure is recorded


<!-- @trace
source: auto-review
updated: 2026-08-15
code:
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/agent-core/src/agent.rs
  - docs/tools.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/event.rs
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: Auto reviewer is toolless and secrets are protected

The auto-review model call SHALL receive no tool specifications and SHALL NOT be able to execute tools, alter candidate arguments, or persist data. Secrets and sensitive values SHALL be redacted before entering the review prompt, reviewer reason, or audit record. A redaction failure SHALL deny the candidate.

#### Scenario: reviewer cannot call a tool

- **WHEN** the auto-review model response contains a tool call instead of the required decision object
- **THEN** the candidate tool does not execute and the response is recorded as a review protocol violation

#### Scenario: secret is absent from review output

- **WHEN** a candidate argument contains an API key or token
- **THEN** the review prompt and audit record contain a redacted representation rather than the secret value


<!-- @trace
source: auto-review
updated: 2026-08-15
code:
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/agent-core/src/agent.rs
  - docs/tools.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/event.rs
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: Auto review records an auditable outcome

Each auto-reviewed candidate SHALL record the decision, risk class, tool name, reviewer provider/model label, danger-signal codes, latency, and sanitized reason in the per-device audit trail. A denied candidate SHALL also record the existing denied-tool outcome. Raw candidate arguments, prompt text, tokens, API keys, and passwords SHALL NOT be persisted.

#### Scenario: approved review is auditable

- **WHEN** the reviewer approves a mutate or critical candidate and the tool completes
- **THEN** the audit trail contains the review decision and the tool outcome without raw secrets

#### Scenario: denied review is auditable

- **WHEN** the reviewer denies a candidate or the review fails closed
- **THEN** the audit trail identifies the denial or failure category and the candidate tool is recorded as not executed

<!-- @trace
source: auto-review
updated: 2026-08-15
code:
  - crates/fleety-server/src/storage.rs
  - prompts/policy.md
  - crates/agent-core/src/approval.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/agent-core/src/agent.rs
  - docs/tools.md
  - crates/agent-core/src/subagent.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-tools/src/lib.rs
  - crates/agent-core/src/tools.rs
  - crates/fleety-server/src/main.rs
  - docs/env.md
  - crates/agent-core/src/lib.rs
  - crates/agent-core/src/event.rs
  - crates/fleety-server/src/auto_review.rs
  - crates/fleety-server/src/bridge.rs
  - crates/fleety-server/src/scheduler.rs
-->