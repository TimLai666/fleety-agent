## ADDED Requirements

### Requirement: Manage and call MCP servers across builtin and installed tiers

The system SHALL provide `mcp_list`, `mcp_add`, `mcp_remove`, and `mcp_call`. The MCP runtime SHALL merge two tiers into one server surface: a **builtin** tier seeded by the runtime (`builtin.json`) and a **user-installed** tier (`installed.json`). `mcp_call` SHALL invoke a named `tool` on a named `server` with `arguments` and return its result, regardless of which tier the server belongs to. `mcp_add` SHALL write only to the user-installed tier, and `mcp_remove` SHALL remove only user-installed entries and SHALL NOT remove builtin ones.

#### Scenario: call a tool on a server from either tier

- **WHEN** `mcp_call` names a configured server (builtin or installed) and one of its tools with arguments
- **THEN** the runtime starts/uses that server and returns the tool's result

#### Scenario: removing a builtin server is refused

- **WHEN** `mcp_remove` names a builtin-tier server
- **THEN** the builtin entry is preserved and the call does not delete it

### Requirement: Built-in ddgs web search

`ddgs` SHALL be shipped as a **builtin-tier** MCP (seeded into `builtin.json`, not the user-installed tier), providing `search_text`, `search_images`, `search_news`, `search_videos`, `search_books`, and `extract_content`. The runtime SHALL seed the ddgs entry at boot, and (unless auto-install is disabled) SHALL install it when missing and keep it upgraded alongside the server. The agent SHALL reach these search tools through the same `mcp_call` surface as any other MCP server.

#### Scenario: ddgs is available without manual setup

- **WHEN** the server boots with ddgs auto-install enabled and ddgs is not yet installed
- **THEN** the runtime installs ddgs and seeds its builtin-tier entry so `search_text` is callable via `mcp_call`
