## Why

`config provider edit` 的互動編輯器只能新增／刪除 provider：要改既有 provider 的 key 或 model 得先刪再重加，若被 group／role 引用還得先解綁；TUI 也缺 group remove 與 role unset（子命令都有）；新增走逗號分隔單行輸入、無欄位級驗證；`d` 刪除毫無確認，一鍵即毀。

## What Changes

- 在互動編輯器就地編輯既有 provider 的欄位（name 以外的 base_url／model／key），沿用 `config provider set` 的「只改有給的欄位、其餘保留」語意，因此被 group／role 引用的 provider 可直接改而不必先解綁。
- 在 Browse 鍵盤操作加入 group remove 與 role unset，行為對齊 `config group remove` / `config role unset`（移除仍被 role 引用的 group 會被拒絕並指名引用者）。
- 新增／編輯 provider 改為逐欄提示輸入（name → base_url → model → key），取代逗號分隔單行；必填欄位（name／base_url／model）留空時以指名該欄位的錯誤訊息擋下，不寫入。
- `d` 刪除 provider 前先進入確認提示（指名該 provider），確認才移除，取消則配置不變。
- 更新 Browse 狀態列說明字串，讓新按鍵（編輯、group remove、role unset）可見。

## Non-Goals

- 不做遠端／過線的互動編輯：`config provider edit` 維持 local + TTY only（`is_interactive_edit` 現況），這是後續工作。
- 不改 `providers.toml` 格式、不動 `provider|group|role` 子命令的介面或 `fleety-tools` 的解析／寫入層。
- 逐欄輸入只覆蓋單行輸入原本就有的四個欄位（name／base_url／model／key）；stream／modalities／effort／auth 等進階欄位仍只透過子命令設定。
- 不把 group set（建立／取代）的單行 member 輸入改成逐欄式；本次只補 group remove。
- 不新增 provider schema 欄位。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- provider-config-surface: 為 TTY 互動編輯器補上「就地編輯既有 provider」「group remove／role unset」「逐欄輸入＋必填驗證」「刪除前確認」四項行為。

## Impact

- Affected specs: provider-config-surface
- Affected code:
  - Modified: crates/fleety-cli/src/provider_tui.rs