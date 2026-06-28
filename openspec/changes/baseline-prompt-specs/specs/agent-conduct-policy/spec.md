## ADDED Requirements

### Requirement: Full access by default

The agent SHALL operate with full access by default: mutating tools run without per-call approval. Per-call approval gating SHALL apply only when the runtime policy is `require_approval`.

#### Scenario: default posture runs mutations directly

- **WHEN** the runtime policy is the default and a mutating tool is invoked
- **THEN** the action runs without a per-call approval prompt, recorded in the audit log

### Requirement: Physical-presence actions require co-location

An action that needs physical presence at a place (operating local-only hardware, anything bound to a site) SHALL be performed only through a device that is actually located at that site, not routed to an arbitrary device.

#### Scenario: physical action routes to an on-site device

- **WHEN** a task needs physical presence at a site
- **THEN** the agent selects a device located at that site rather than a remote one

### Requirement: Intrusive UI control prefers the least-intrusive tier

When driving an interface, the agent SHALL prefer the least-intrusive capable tier in the order dedicated API/MCP, then browser CDP, then native computer-use, and SHALL fall to computer-use only when no higher tier can accomplish the task. Taking a screenshot to observe a screen SHALL be exempt from this ranking as a low-impact read.

#### Scenario: computer-use is the last resort

- **WHEN** a task can be done through a dedicated API/MCP or browser CDP
- **THEN** the agent uses that tier rather than native mouse/keyboard control
- **WHEN** only a screen observation is needed
- **THEN** a screenshot is allowed without escalating through the ranking

### Requirement: Unattended scheduled runs are mandate-bounded

A scheduled (unattended) run SHALL act only within the mandate and allowed-tools captured when the schedule was created, and SHALL NOT take destructive or outward-facing actions beyond that mandate without it being part of the schedule's intent.

#### Scenario: scheduled run honours its captured mandate

- **WHEN** a schedule fires
- **THEN** the run is constrained to the mandate and allowed_tools recorded at creation time

### Requirement: Mutating actions are audited and reversible

Every mutating action SHALL be recorded to the per-device audit log, and file mutations SHALL be backed up so they can be reverted via rollback.

#### Scenario: a mutation is auditable and reversible

- **WHEN** a file-mutating tool runs
- **THEN** the action is appended to the audit log and a backup is captured for rollback
