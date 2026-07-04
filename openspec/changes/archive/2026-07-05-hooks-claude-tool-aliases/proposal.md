## Why

hooks-compat 的工具比對用**精確工具名**。但 Claude Code 的 hooks matcher 用的是 Claude 的工具名(`Bash`／`Read`／`Write`／`Edit`／`LS`…),而 Fleety 的工具叫 `run_command`／`read_file`／`write_file`／`edit_file`／`list_dir`…。兩者名稱不同,導致真實使用者設定的具名 matcher(如 `"matcher":"Bash"`)在 Fleety **永遠不會命中**——只有 `"*"` 或空 matcher 有效。這讓 Pre/PostToolUse hooks 在實務上幾乎失效,違背「復用發起端已設定 hooks」的初衷。此為端對端校準時發現的實質缺口。

## What Changes

- 在 `hooks_compat` 加一張 Claude→Fleety 工具名別名表(`claude_alias`),涵蓋常見內建工具:`Bash`→`run_command`、`Read`→`read_file`、`Write`→`write_file`、`Edit`／`MultiEdit`→`edit_file`、`LS`→`list_dir`、`Glob`／`Grep`→`search_files`、`WebFetch`→`fetch_url`。
- `matches(matcher, tool_name)` 除了 `*`／空／精確相等外,再認別名:當 `claude_alias(matcher)` 等於該 Fleety 工具名時也命中。維持向後相容(既有 `"*"` 與剛好同名者不受影響)。
- 純比對邏輯變更,不動執行、否決、audit、事件觸發。

## Non-Goals

- 不做完整 Claude 工具集對映(只涵蓋 Fleety 有對應的常見工具);未知 matcher 仍走精確比對。
- 不做反向(Fleety→Claude)或雙向 regex/alternation matcher(仍屬 hooks-compat 既有 Non-Goal)。
- 不改 MCP／plugin 工具的命名。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `hooks-compat`: 工具比對從「僅精確 Fleety 工具名」擴為「精確名 或 Claude 別名對映到該 Fleety 工具」,讓真實 Claude Code 具名 matcher 能命中。

## Impact

- Affected specs: `hooks-compat`(修改「matcher matches by tool name」需求)。
- Affected code:
  - Modified: `crates/fleety-server/src/hooks_compat.rs`(`claude_alias` 別名表 + `matches` 認別名)
  - New: (none)
  - Removed: (none)
