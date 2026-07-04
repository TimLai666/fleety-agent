# mcp-servers Specification

## Purpose

TBD - created by archiving change 'baseline-tool-surface-specs'. Update Purpose after archive.

## Requirements

### Requirement: Manage and call MCP servers across builtin and installed tiers

The system SHALL provide `mcp_list`, `mcp_add`, `mcp_remove`, and `mcp_call`. The MCP runtime SHALL merge two tiers into one server surface: a **builtin** tier seeded by the runtime (`builtin.json`) and a **user-installed** tier (`installed.json`). `mcp_call` SHALL invoke a named `tool` on a named `server` with `arguments` and return its result, regardless of which tier the server belongs to. `mcp_add` SHALL write only to the user-installed tier, and `mcp_remove` SHALL remove only user-installed entries and SHALL NOT remove builtin ones.

#### Scenario: call a tool on a server from either tier

- **WHEN** `mcp_call` names a configured server (builtin or installed) and one of its tools with arguments
- **THEN** the runtime starts/uses that server and returns the tool's result

#### Scenario: removing a builtin server is refused

- **WHEN** `mcp_remove` names a builtin-tier server
- **THEN** the builtin entry is preserved and the call does not delete it


<!-- @trace
source: baseline-tool-surface-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - CLAUDE.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-discuss.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-ask.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-drift/SKILL.md
-->

---
### Requirement: Built-in ddgs web search

`ddgs` SHALL be shipped as a **builtin-tier** MCP (seeded into `builtin.json`, not the user-installed tier), providing `search_text`, `search_images`, `search_news`, `search_videos`, `search_books`, and `extract_content`. The runtime SHALL seed the ddgs entry at boot, and (unless auto-install is disabled) SHALL install it when missing and keep it upgraded alongside the server. The agent SHALL reach these search tools through the same `mcp_call` surface as any other MCP server.

#### Scenario: ddgs is available without manual setup

- **WHEN** the server boots with ddgs auto-install enabled and ddgs is not yet installed
- **THEN** the runtime installs ddgs and seeds its builtin-tier entry so `search_text` is callable via `mcp_call`

<!-- @trace
source: baseline-tool-surface-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - CLAUDE.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .agents/skills/spectra-debug/SKILL.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .spectra.yaml
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .agents/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-discuss.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-ask.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-drift/SKILL.md
-->

---
### Requirement: Enabled plugin MCP servers are available per-conversation

The MCP servers declared by each enabled Claude Code plugin on the originating device SHALL be available to that conversation through `mcp_call`, merged **per-conversation** into the server list rather than into the global installed set. A plugin's Claude Code MCP shape (`{ name: { command, args, env } }`) SHALL be converted to the runtime's server shape (`{ name, command, args }`). Same-name precedence SHALL be project plugin > user plugin > installed > builtin. Conversion and merge SHALL be best-effort: a malformed plugin MCP config SHALL be skipped without aborting the conversation.

#### Scenario: an enabled plugin's MCP server is callable in the conversation

- **WHEN** a same-host conversation binds and an enabled plugin declares an MCP server
- **THEN** that server appears in `mcp_list` for that conversation and can be invoked via `mcp_call`

#### Scenario: plugin MCP servers are per-conversation, not global

- **WHEN** one conversation reuses an enabled plugin's MCP server
- **THEN** the server is visible only to that conversation and is not written into the global installed MCP config

#### Scenario: same-named MCP server follows plugin precedence

- **WHEN** a server name exists in both a project plugin and the installed set
- **THEN** the project plugin's server is the one served

<!-- @trace
source: claude-plugin-declarative-reuse
updated: 2026-07-04
code:
  - crates/fleety-server/src/subagent.rs
  - crates/fleety-server/src/skills.rs
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/skill_sources.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-server/src/plugin_sources.rs
  - crates/fleety-server/src/mcp.rs
  - crates/fleety-server/src/main.rs
-->

---
### Requirement: Codex config.toml MCP servers are available per-conversation

The MCP servers declared in the originating device's `~/.codex/config.toml` (`[mcp_servers.<name>]` with `command` / `args`) SHALL be available to a same-host conversation through `mcp_call`, merged per-conversation at **user scope** — below project/user plugin servers, above installed and builtin by name. Conversion and merge SHALL be best-effort: a missing or malformed `config.toml`, or an entry without a command, SHALL be skipped without aborting the conversation.

#### Scenario: a Codex-declared MCP server is callable in the conversation

- **WHEN** a same-host conversation binds and `~/.codex/config.toml` declares an MCP server
- **THEN** that server appears in `mcp_list` for that conversation and can be invoked via `mcp_call`

#### Scenario: Codex MCP ranks below plugin servers, above installed

- **WHEN** a server name exists in both a Codex config entry and the installed set
- **THEN** the Codex entry is served; when it also exists in a plugin, the plugin server wins

<!-- @trace
source: codex-declarative-reuse
updated: 2026-07-04
code:
  - crates/fleety-server/src/instructions.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-server/src/codex_sources.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/mcp.rs
-->