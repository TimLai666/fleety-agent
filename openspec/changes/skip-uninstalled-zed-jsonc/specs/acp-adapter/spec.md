## MODIFIED Requirements

### Requirement: The CLI configures editors to launch the agent

The CLI SHALL provide `acp install [<editor>]` to register itself as an ACP agent. With no editor, it SHALL print the generic launch details (the command, `["acp"]`, and an optional `FLEETY_AGENT_URL`) that any ACP-capable editor uses. For a supported editor (`zed`), it SHALL merge an entry into that editor's config pointing at the current binary, preserving the editor's other settings and other agents, backing up the prior file, and SHALL NOT clobber a config it cannot safely parse (e.g. JSONC with comments) — printing the snippet to paste instead. Re-running SHALL overwrite an existing entry (an update, not a duplicate). `fleety update` SHALL inspect an existing Zed settings file without requiring unrelated JSONC to parse; when no `agent_servers.Fleety` entry exists, it SHALL treat the refresh as a no-op. When an entry exists, `fleety update` SHALL re-point it at the current binary only after strict parsing and validation succeed, and SHALL report failure without writing when they do not.

#### Scenario: install configures a supported editor

- **WHEN** the user runs `fleety acp install zed`
- **THEN** an `agent_servers.Fleety` entry pointing at the current binary is written to Zed's settings, the editor's other settings are preserved, and the prior file is backed up

#### Scenario: re-run updates in place

- **WHEN** `fleety acp install zed` is run again (e.g. after the binary moved)
- **THEN** the existing Fleety entry is overwritten with the current binary path rather than duplicated

#### Scenario: an unparseable config is not clobbered

- **WHEN** the editor config cannot be parsed as plain JSON (it has comments)
- **THEN** the config is left unchanged and the entry to add is printed for manual use

##### Example: JSONC install fallback

- **GIVEN** the settings contain `// user comment` before the JSON object
- **WHEN** `fleety acp install zed` edits the settings
- **THEN** the settings remain byte-for-byte unchanged and the manual Fleety entry is printed

#### Scenario: update ignores unrelated JSONC

- **GIVEN** the existing Zed settings contain comments or trailing commas but no `agent_servers.Fleety` entry
- **WHEN** `fleety update` refreshes installed ACP settings
- **THEN** the refresh is treated as a no-op, the settings remain unchanged, and the update continues

#### Scenario: update protects an unparseable installed entry

- **GIVEN** the existing Zed settings appear to contain an `agent_servers.Fleety` entry but cannot be parsed as plain JSON
- **WHEN** `fleety update` refreshes installed ACP settings
- **THEN** the refresh reports an incomplete update and leaves the settings unchanged

<!-- @trace
source: skip-uninstalled-zed-jsonc
updated: 2026-08-04
code:
  - crates/fleety-cli/src/acp.rs
tests:
  - crates/fleety-cli/src/acp.rs
-->
