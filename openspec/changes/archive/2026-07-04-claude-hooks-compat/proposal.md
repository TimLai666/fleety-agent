## Why

相容層前三步已讓對話復用發起端的 instruction 檔、Claude Code 外掛的 skills/MCP、Codex 的 MCP。發起端裝置常在 `~/.claude/settings.json` 與專案 `.claude/settings.json` 設定 hooks（工具執行前後跑的 shell command，如 lint、格式化、稽核、拒絕危險操作）。目前這些 hooks 在 Fleety 對話中完全不生效，使用者在 Claude Code 建立的自動化與防護在跨到 Fleety agent 時全部失效，行為不一致且少了一層安全檢查。此為相容層的第四步、也是唯一會「自動執行任意 shell」的一步，安全模型須明確。

## What Changes

- 新增 `hooks_compat` 模組：純函式解析發起端的 Claude Code `settings.json` hooks（使用者 `~/.claude/settings.json` 與專案 `.claude/settings.json`），支援 `hooks.PreToolUse[]` 與 `hooks.PostToolUse[]`，每項含 `matcher`（工具名 pattern）與 `hooks[].command`（shell）。解析全程 best-effort：缺檔、壞 JSON、格式不符即略過該來源，不中斷對話。
- 在 conn 層以 tool-wrapper 包裝該對話的工具 registry：工具執行「前」跑 matcher 匹配的 PreToolUse hooks、執行「後」跑 PostToolUse hooks。不動 agent-core 核心。
- 事件對映（首版）：Fleety 工具執行前 = PreToolUse、執行後 = PostToolUse。matcher 首版用簡單工具名比對（精確或萬用 `*`）。
- hook 一律在 **origin device** 執行：同主機用本機 shell；跨裝置經 `device_exec` 的命令執行送到發起端跑（hook 屬於發起端，在其環境跑語意才正確）。
- PreToolUse 否決：hook command 非零 exit（或 deny 輸出）即比照現有 `ApprovalGate` 否決該工具，agent 收到被 hook 拒絕的工具結果，工具不執行。
- 每次 hook shell 執行（命令、結果、來源 scope）寫入 Fleety 既有 audit log。
- **BREAKING**（行為面，非 API）：hooks 預設開（opt-out）。使用者級 hooks 隨對話自動生效；專案級 hooks 也預設開，但可用環境變數 `FLEETY_DISABLE_PROJECT_HOOKS=1` 單獨關閉，且一律在 audit 標記為 `project-sourced`（專案 hooks 可能來自不可信 repo，屬供應鏈風險）。

## Non-Goals

- 不做 `UserPromptSubmit`、`Stop`、`SubagentStop`、`SessionStart` 等其他 hook 事件（列後續）。
- 不做 Codex hooks（Codex 無 hooks 機制）。
- 不引入 Fleety 自建的 hook 安裝／設定 UI，也不讓 agent 產生自己的 hook；只復用發起端「已設定」的 hooks。
- 不做 Claude Code hooks 的進階 matcher 語法（正規表示式群組、工具輸入條件比對等），首版僅工具名精確／萬用比對。
- 不保證與 Claude Code hook 執行協定（stdin 傳入的 JSON schema、exit code 細部語意、hook 輸出格式）逐位相容；以 best-effort 對映，細節列 Open Question 待實測校準。

## Capabilities

### New Capabilities

- `hooks-compat`: 對話復用發起端 Claude Code 已設定的 PreToolUse/PostToolUse hooks，於 origin device 執行，PreToolUse 可否決工具，全程 audit，opt-out 安全模型含專案 hooks 單獨關閉開關。

### Modified Capabilities

(none)

## Impact

- Affected specs: 新增 `hooks-compat`。
- Affected code:
  - New: `crates/fleety-server/src/hooks_compat.rs`
  - Modified: `crates/fleety-server/src/conn.rs`（tool-wrapper 執行點、跨裝置 hook 執行、否決與 audit 串接）、`crates/fleety-server/src/main.rs`（模組宣告）
  - Removed: (none)
