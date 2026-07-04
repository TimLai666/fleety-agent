## ADDED Requirements

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
