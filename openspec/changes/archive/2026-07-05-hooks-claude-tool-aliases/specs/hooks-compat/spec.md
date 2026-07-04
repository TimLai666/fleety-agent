## MODIFIED Requirements

### Requirement: Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks

The runtime SHALL, for a conversation, discover the originating device's Claude Code `PreToolUse` and `PostToolUse` hooks declared in the user settings (`~/.claude/settings.json`) and the project settings (project `.claude/settings.json`), and run the matching hooks around that conversation's tool calls. Hooks SHALL be parsed from the `hooks.PreToolUse` and `hooks.PostToolUse` arrays, where each array element carries a `matcher` (a tool-name pattern; absent or empty means match-all) and a `hooks` list whose entries of `type == "command"` provide the shell `command`. Parsing SHALL be best-effort: a missing or malformed settings file, an absent `hooks` section, or an entry without a command SHALL be skipped and SHALL NOT abort the conversation.

Hook matching SHALL use tool-name comparison: a `matcher` of `*` (or empty) matches every tool; otherwise the matcher SHALL match a tool whose Fleety name equals it OR whose Fleety name is the mapping of the matcher when the matcher is a known Claude Code tool name. Because Claude Code hooks reference Claude's tool names (e.g. `Bash`, `Read`, `Write`, `Edit`, `LS`) while the runtime's tools use different names (e.g. `run_command`, `read_file`, `write_file`, `edit_file`, `list_dir`), the runtime SHALL map the common Claude Code built-in tool names to their runtime equivalents so a named matcher fires on the corresponding runtime tool. An unknown matcher SHALL fall back to exact-name comparison. Advanced matcher syntax (regular expressions, tool-input predicates) remains out of scope.

#### Scenario: PreToolUse and PostToolUse hooks are parsed

- **WHEN** a conversation's settings declare `hooks.PreToolUse` and `hooks.PostToolUse` entries with a `matcher` and a `command`
- **THEN** those hooks are collected, each tagged with its source scope (user or project) and its event

#### Scenario: malformed settings are skipped best-effort

- **WHEN** a settings file is missing, is not valid JSON, or has no `hooks` section
- **THEN** no hooks are collected from that source and the conversation proceeds without error

#### Scenario: matcher matches by tool name

- **WHEN** a hook's `matcher` is `*` or empty
- **THEN** it matches every tool
- **WHEN** a hook's `matcher` is a specific runtime tool name
- **THEN** it matches only the tool whose name equals that matcher

#### Scenario: a Claude Code tool-name matcher matches the runtime tool

- **WHEN** a hook's `matcher` is a known Claude Code tool name such as `Bash`, `Read`, `Write`, `Edit`, or `LS`
- **THEN** it matches the corresponding runtime tool (`run_command`, `read_file`, `write_file`, `edit_file`, `list_dir` respectively)

#### Scenario: an unknown matcher falls back to exact match

- **WHEN** a hook's `matcher` is not `*`, not a runtime tool name, and not a known Claude Code tool name
- **THEN** it matches only a tool whose name exactly equals the matcher
