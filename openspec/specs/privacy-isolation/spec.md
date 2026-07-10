# privacy-isolation Specification

## Purpose

TBD - created by archiving change 'privacy-isolation'. Update Purpose after archive.

## Requirements

### Requirement: The acting user is a hard privacy boundary

Every read of conversations, recall, and per-user memory SHALL be scoped to the acting user through a data-layer guard: a turn SHALL only read the acting user's data, plus anything explicitly granted to it. Cross-user access SHALL be default-deny. Enforcement SHALL be at the data layer, not only in the prompt. Guest SHALL have no access to any real user's private data.

#### Scenario: a turn reads only the acting user's data

- **WHEN** the acting user is A and the agent reads conversations or memory
- **THEN** it sees A's data and never B's

#### Scenario: cross-user is denied by default

- **WHEN** a turn attempts to read another user's data with no grant
- **THEN** the access is denied

#### Scenario: Guest reads no private data

- **WHEN** the acting user is Guest
- **THEN** no real user's conversations, recall, or memory are readable


<!-- @trace
source: privacy-isolation
updated: 2026-06-29
code:
  - crates/fleety-server/src/privacy.rs
  - prompts/policy.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - prompts/memory.md
-->

---
### Requirement: No disclosure of another user's content, timing, or existence

The agent SHALL NOT disclose another user's information without that user's authorization — not the content, not when they used the system, and not whether they exist or whether a topic was discussed with them. A denied cross-user access SHALL return a uniform response that does not distinguish "no such data" from "exists but forbidden", so the refusal itself reveals nothing.

#### Scenario: existence is not leaked by a refusal

- **WHEN** the agent is asked about another user's data it may not access
- **THEN** it responds in a uniform "not available to you" way that does not reveal whether that user or that data exists

#### Scenario: timing is not leaked

- **WHEN** the agent is asked when another user last used the system, without authorization
- **THEN** it does not reveal that timing


<!-- @trace
source: privacy-isolation
updated: 2026-06-29
code:
  - crates/fleety-server/src/privacy.rs
  - prompts/policy.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - prompts/memory.md
-->

---
### Requirement: Cross-user access requires an explicit grant

Access to another user's data SHALL require an explicit grant from that user, covering a defined scope; absent a matching grant, access SHALL be denied. Grants SHALL be consulted by the data-layer guard. A corrupt or unreadable grant store SHALL fail closed (deny).

#### Scenario: granted access within scope is allowed

- **WHEN** user A has granted the acting principal access to a scope, and the requested data is within it
- **THEN** the access is allowed

#### Scenario: access outside the grant is denied

- **WHEN** the requested data is outside any grant the acting principal holds
- **THEN** the access is denied


<!-- @trace
source: privacy-isolation
updated: 2026-06-29
code:
  - crates/fleety-server/src/privacy.rs
  - prompts/policy.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - prompts/memory.md
-->

---
### Requirement: Conversations are stored per user, with device recorded, migrated losslessly

Conversations SHALL be stored under the owning user (`users/<user>/conversations/`), with each event recording the device it occurred on. Existing per-device conversations SHALL be migrated once under their device's owner (or a reserved unattributed bucket when the device has no owner), the migration SHALL be lossless and idempotent, and resume/lookup by conversation id SHALL continue to work via an id-to-owner index.

#### Scenario: existing conversations migrate under their owner

- **WHEN** the system first runs after this change and a device has an owner
- **THEN** that device's conversations are placed under the owner with the device recorded per event, and nothing is lost

#### Scenario: resume still works after migration

- **WHEN** a client resumes a conversation by id after migration
- **THEN** the id resolves to the right user's conversation and replays correctly

#### Scenario: a device with no owner

- **WHEN** a device without an owner is migrated
- **THEN** its conversations go to a reserved unattributed bucket rather than being lost or attributed to a real user

<!-- @trace
source: privacy-isolation
updated: 2026-06-29
code:
  - crates/fleety-server/src/privacy.rs
  - prompts/policy.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - prompts/memory.md
-->

---
### Requirement: Tool-result retrieval and audit listing respect the user boundary

Retrieving a tool result (`fetch_tool_result`) and listing audit history
(`history_list`) SHALL be confined to conversations the acting user can access.
An id that exists but belongs to another user's conversation SHALL be reported as
not found, with no indication that it exists (consistent with the user-as-privacy-
boundary, no-leak rule). The audit listing SHALL return only entries from the
acting user's accessible conversations.

#### Scenario: cannot fetch another user's tool result

- **WHEN** acting user A calls `fetch_tool_result` with an id from user B's conversation
- **THEN** it is reported as not found, with no hint that the id exists

#### Scenario: audit listing is scoped to the acting user

- **WHEN** the acting user lists audit history on a shared device
- **THEN** only that user's accessible entries are returned, not other users' tool output

<!-- @trace
source: retrievable-tool-results
updated: 2026-06-29
code:
  - crates/agent-core/src/compress.rs
  - docs/env.md
  - crates/fleety-server/src/conn.rs
  - crates/agent-core/src/event.rs
  - crates/agent-core/src/agent.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/tools.rs
-->

---
### Requirement: A user can revoke and list the grants they made

The data owner SHALL be able to revoke a cross-user grant they previously made and to list their outstanding grants. A `revoke_access` tool SHALL remove grants matching the given grantee, narrowed by an optional scope (exact match); when the scope is omitted, every grant the owner made to that grantee SHALL be removed. Revocation SHALL take effect immediately, so the data-layer guard denies the revoked access on its next decision. A `list_access` tool SHALL return the grants the acting user currently holds as owner (grantee and scope). Guest SHALL NOT revoke any grant and SHALL receive an empty list. Revoking a grant that does not exist SHALL succeed and report zero grants removed, revealing nothing about other users. The grant store SHALL be updated under the same lock as grant creation so concurrent grant and revoke operations MUST NOT clobber each other.

#### Scenario: revoking a grant removes access

- **WHEN** owner A revokes the grant that let B access A's scope, and B then attempts that access
- **THEN** the revocation removes the grant and B's access is denied

#### Scenario: revoking without a scope removes all grants to that grantee

- **WHEN** owner A revokes B with no scope while A holds multiple scoped grants to B
- **THEN** every grant A made to B is removed and B retains no access to A's data

#### Scenario: listing shows the owner's outstanding grants

- **WHEN** the acting user lists their access grants
- **THEN** each grant they made is returned with its grantee and scope, and no other owner's grants appear

#### Scenario: guest cannot revoke or enumerate grants

- **WHEN** the acting principal is Guest and attempts to revoke or list grants
- **THEN** the revoke is refused and the list is empty

#### Scenario: revoking a non-existent grant reveals nothing

- **WHEN** owner A revokes a grantee or scope that A never granted
- **THEN** the operation succeeds reporting zero grants removed and discloses nothing about whether that grantee exists

<!-- @trace
source: grant-access-revoke
updated: 2026-07-10
code:
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/provider_tui.rs
  - docs/env.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/privacy.rs
  - scripts/install.sh
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/scheduler.rs
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/config.rs
  - README.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->