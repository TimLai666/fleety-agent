## ADDED Requirements

### Requirement: Discover and reuse enabled Claude Code plugins

The runtime SHALL, for a same-host conversation, discover the originating device's **enabled** Claude Code plugins and make their declarative resources (skills and MCP servers) available to that conversation. Enabled plugins SHALL be determined by parsing the `enabledPlugins` setting in the project `.claude/settings.json` and the user `~/.claude/settings.json`, tolerating both an object form (`{ "name": true }`, taking the entries whose value is true) and an array form (`[ "name" ]`). For each enabled plugin located under the Claude Code plugins directory, its `skills/` directory and its MCP server configuration SHALL be collected and tagged with the plugin's scope (project or user). All parsing SHALL be best-effort: a missing or malformed settings file, an absent `enabledPlugins`, or a missing plugin directory SHALL cause that source to be skipped and SHALL NOT abort the conversation. A cross-device or absent origin SHALL contribute no plugin sources (the first release is same-host only).

#### Scenario: enabled plugins are parsed from settings

- **WHEN** `enabledPlugins` is present as an object with some entries true, or as an array
- **THEN** exactly the enabled plugin names are collected

#### Scenario: a disabled plugin is not reused

- **WHEN** a plugin is present on disk but its `enabledPlugins` entry is false (or absent)
- **THEN** its skills and MCP servers are not collected

#### Scenario: malformed settings are skipped best-effort

- **WHEN** a settings file is missing or its JSON is malformed
- **THEN** that source contributes nothing and the conversation proceeds without error

#### Scenario: cross-device origin contributes no plugin sources

- **WHEN** the origin is another device or absent
- **THEN** no plugin skills or MCP servers are collected for that conversation
