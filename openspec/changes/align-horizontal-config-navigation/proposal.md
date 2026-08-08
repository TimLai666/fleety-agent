## Why

設定介面的四個選擇器把選項橫向排列，卻只接受上下方向鍵，讓視覺線索與操作方向互相矛盾。橫向控制應直接回應左右鍵，同時保留既有上下鍵，避免改變熟悉目前操作的使用者流程。

## What Changes

- 橫向排列的 Provider 類型、模型角色、Codex OAuth 動作與未儲存變更選擇器，將左右鍵作為主要前後移動操作。
- 上下鍵保留為相容別名，既有鍵盤操作不會失效。
- 橫向選擇器的固定提示改為顯示左右鍵，並以測試鎖定鍵盤行為、邊界與呈現方向。
- 真正的縱向 Provider、模型與設定列清單維持上下鍵操作。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `interactive-config-panel`: 橫向選擇器的導覽方向與畫面排列一致，且保留既有相容操作。

## Impact

- Affected specs: `interactive-config-panel`
- Affected code:
  - Modified: `crates/fleety-cli/src/provider_tui.rs`
  - New: (none)
  - Removed: (none)
