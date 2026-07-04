## ADDED Requirements

### Requirement: Codex config.toml MCP servers are available per-conversation

The MCP servers declared in the originating device's `~/.codex/config.toml` (`[mcp_servers.<name>]` with `command` / `args`) SHALL be available to a same-host conversation through `mcp_call`, merged per-conversation at **user scope** — below project/user plugin servers, above installed and builtin by name. Conversion and merge SHALL be best-effort: a missing or malformed `config.toml`, or an entry without a command, SHALL be skipped without aborting the conversation.

#### Scenario: a Codex-declared MCP server is callable in the conversation

- **WHEN** a same-host conversation binds and `~/.codex/config.toml` declares an MCP server
- **THEN** that server appears in `mcp_list` for that conversation and can be invoked via `mcp_call`

#### Scenario: Codex MCP ranks below plugin servers, above installed

- **WHEN** a server name exists in both a Codex config entry and the installed set
- **THEN** the Codex entry is served; when it also exists in a plugin, the plugin server wins
