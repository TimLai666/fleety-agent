## Why

從 Settings 的 Providers & Models 頁面開啟 Provider editor 時，CLI 會先送出離開 alternate screen 的控制序列，完成連線與 Provider snapshot 載入後才重新進入 alternate screen。圖形終端因此會真的跳回 scrollback；若使用者緊接著按 `a`，Add Provider 畫面可能尚未出現，既有純狀態測試也無法觀察這個終端生命週期錯誤。

## What Changes

- Settings 進入 Provider editor 時沿用同一個 full-screen terminal，不在正常的 Enter 與 `a` 流程中離開再重進 alternate screen。
- Provider editor 保留獨立命令啟動時自行初始化與還原終端的能力。
- OAuth 等確實需要 plain terminal 的流程仍可明確暫停 full-screen terminal，完成後再恢復。
- 新增終端層級回歸測試，驗證從 Providers & Models 按 Enter 後再按 `a` 會顯示 Add Provider，且正常交接不產生 LeaveAlternateScreen 後緊接 EnterAlternateScreen 的序列。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `terminal-workspace`: Settings 與其巢狀 Provider editor 必須共享連續的 full-screen terminal session，正常導航不得暴露 primary scrollback。

## Impact

- Affected specs: terminal-workspace
- Affected code:
  - Modified: crates/fleety-cli/src/config_panel.rs
  - Modified: crates/fleety-cli/src/config.rs
  - Modified: crates/fleety-cli/src/provider_tui.rs
  - Modified: crates/fleety-cli/src/test_terminal.rs
  - New: openspec/changes/preserve-provider-terminal-screen/specs/terminal-workspace/spec.md
