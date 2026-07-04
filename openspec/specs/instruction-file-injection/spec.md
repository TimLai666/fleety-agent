# instruction-file-injection Specification

## Purpose

TBD - created by archiving change 'instruction-file-injection'. Update Purpose after archive.

## Requirements

### Requirement: Runtime injects project and user instruction files into a conversation

The runtime SHALL, when a conversation binds, collect and inject the `AGENTS.md` and `CLAUDE.md` instruction files found at each level from the project root down to the origin `cwd`, plus the originating device's user-global instruction files (`~/.claude/CLAUDE.md` and `~/.agents/AGENTS.md`), into that conversation's context. The injected content SHALL be ordered shallow-to-deep (so deeper, more specific files follow and refine the root ones) with the user-global files included. When the origin device is the server host, the files SHALL be read locally; when the origin device is another device, the files SHALL be read from that device via `device_exec`. The injection SHALL use the ephemeral per-turn preamble (not written to conversation history) so it survives long-context compaction. The injected instruction content SHALL be scoped to that conversation only and SHALL NOT be visible to other conversations. This injection augments, and SHALL NOT replace, the agent's ability to read instruction files on demand.

#### Scenario: same-host conversation injects layered project and user files

- **WHEN** a conversation binds on the server-host device with cwd D under project root R
- **THEN** its context includes the `AGENTS.md` / `CLAUDE.md` at each level from R down to D, plus the device's `~/.claude/CLAUDE.md` and `~/.agents/AGENTS.md`, ordered shallow-to-deep

#### Scenario: cross-device conversation reads the origin device's files

- **WHEN** the origin is another device X with cwd D
- **THEN** the runtime reads the layered project and user-global instruction files from X via `device_exec` and injects their content, rather than reading the server's local files

#### Scenario: injection is scoped to the conversation

- **WHEN** two conversations bind to different projects or devices
- **THEN** each conversation's context contains only its own injected instruction files, with no bleed between them

#### Scenario: injection survives compaction

- **WHEN** a conversation runs many turns and earlier history is compacted
- **THEN** the injected instruction files are still present each turn, because they are re-injected via the ephemeral preamble rather than stored in the compacted history


<!-- @trace
source: instruction-file-injection
updated: 2026-07-04
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/src/compress.rs
-->

---
### Requirement: Injection is deduplicated and size-bounded

The runtime SHALL inject each instruction-file path at most once per conversation (deduplication across the collected set). It SHALL enforce a per-file byte limit and a per-injection total-byte limit; content exceeding a limit SHALL be truncated with an explicit truncation marker. A path with no file present SHALL be skipped. When a cross-device read fails or the origin device is offline, that source SHALL be skipped with a short note and SHALL NOT abort the conversation. The limits SHALL be named constants overridable by environment variable.

#### Scenario: a duplicate path is injected once

- **WHEN** the collected set would include the same instruction-file path more than once
- **THEN** that path's content appears in the injection exactly once

#### Scenario: oversized content is truncated

- **WHEN** an instruction file exceeds the per-file byte limit
- **THEN** its content is truncated and marked as truncated, and the conversation proceeds

#### Scenario: a cross-device read failure is not fatal

- **WHEN** the origin device is offline or a cross-device read errors
- **THEN** that source is skipped with a short note and the rest of the injection and the turn proceed

##### Example: collection boundary cases

| Situation | Result |
| --------- | ------ |
| same path from two layers | injected once |
| file absent at a layer | that layer skipped |
| file over per-file limit | truncated with marker |
| total over injection limit | later files truncated/omitted with marker |
| origin device offline | cross-device sources skipped, note added |


<!-- @trace
source: instruction-file-injection
updated: 2026-07-04
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/src/compress.rs
-->

---
### Requirement: Out-of-tree instruction files are read by the agent on demand

The runtime SHALL auto-inject the initial tree (project root down to cwd) plus the user-global files, re-read each turn. For directories **outside** that initial tree, the runtime SHALL NOT auto-inject; instead the agent reads their `AGENTS.md` / `CLAUDE.md` on demand under the standing instruction to read a directory's own instruction files before acting there. This bounds the runtime's automatic injection to the highest-value set (the origin tree + user-global — the part the agent cannot be relied on to fetch every time) and avoids a per-turn full-tree rescan, while still covering out-of-tree directories through the agent's own reads.

#### Scenario: initial tree injected automatically

- **WHEN** a conversation binds with cwd D under project root R
- **THEN** the instruction files from R down to D are auto-injected, re-read each turn

#### Scenario: out-of-tree files are read by the agent

- **WHEN** the agent works in a directory E outside the initial R-to-D tree
- **THEN** the runtime does not auto-inject E's chain, and the agent reads E's own `AGENTS.md` / `CLAUDE.md` on demand

<!-- @trace
source: instruction-file-injection
updated: 2026-07-04
code:
  - crates/fleety-server/src/main.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/tools.rs
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/src/workspace.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/src/compress.rs
-->