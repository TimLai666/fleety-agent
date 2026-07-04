# codex-compat Specification

## Purpose

TBD - created by archiving change 'codex-declarative-reuse'. Update Purpose after archive.

## Requirements

### Requirement: Reuse an originating device's Codex declarative resources

The runtime SHALL, for a same-host conversation, read the originating device's `~/.codex/config.toml` and make its declared MCP servers available to that conversation (per-conversation, user scope), and SHALL include the originating device's `~/.codex/AGENTS.md` among the conversation's user-global instruction files. Parsing SHALL be best-effort: a missing or malformed `config.toml`, an absent `mcp_servers` table, or a missing `AGENTS.md` SHALL be skipped and SHALL NOT abort the conversation. A cross-device or absent origin SHALL contribute no Codex sources (first release is same-host only).

#### Scenario: Codex MCP servers are parsed and offered

- **WHEN** a same-host conversation binds and `~/.codex/config.toml` declares `[mcp_servers.<name>]` entries
- **THEN** those servers are offered to that conversation (user scope)

#### Scenario: malformed Codex config is skipped best-effort

- **WHEN** `~/.codex/config.toml` is missing or its TOML is malformed
- **THEN** no Codex MCP source is added and the conversation proceeds without error

#### Scenario: cross-device origin contributes no Codex sources

- **WHEN** the origin is another device or absent
- **THEN** no Codex MCP servers or `AGENTS.md` are collected for that conversation

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