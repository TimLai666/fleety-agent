## Problem

Settings 面板開啟後，Connection 區把 current profile 從 B 切到 A，只會更新記憶體與 connections.toml，既有 WebSocket 仍連著 B。使用者若在同一個面板切到 Server 或 Daemon 區套用設定，畫面顯示的 current profile 與真正接收修改的 server 可能不同。

## Root Cause

run_settings 在進入面板前只建立一次連線與 snapshot。Connection 區的 current profile 變更沒有觸發關閉舊連線、依新 profile 解析憑證與 fingerprint、重新握手或刷新遠端區域狀態。

## Proposed Solution

- current profile 儲存成功後，關閉舊連線並使用剛選定的 profile 建立新連線，不重新讀取可能已改變的全域 current。
- 切換時清除舊 server 與 daemon 的 snapshot、revision 及 staged edits，避免把 B 的狀態或修改帶到 A。
- 新連線完成 Welcome 後，分別重新載入 A 的 Server snapshot 與目前裝置在 A 上的 Daemon snapshot。
- 重連或任一 snapshot 失敗時，對應遠端區域顯示 unavailable，且不得保留或使用舊連線。
- Connection 與 CLI 區在遠端重連失敗時仍保持可用。

## Success Criteria

- 從 B 切換並儲存 A 後，同一個面板內的下一次 Server apply 只會送到 A。
- Daemon 區只顯示並修改 A 所管理的目前裝置 daemon。
- B 的 staged edits、revision 與 snapshot 不會出現在 A。
- A 無法連線時，Server 與 Daemon 區顯示 unavailable，且不會回退到 B 或直接修改遠端設定檔。
- 自動測試可重現舊連線仍存活的案例，並證明切換後舊 sender 不再接收 apply。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- interactive-config-panel: current profile 儲存後必須在同一面板內安全重連，重新載入 owner snapshots，並隔離舊 server 的狀態與 staged edits。

## Impact

- Affected specs: interactive-config-panel
- Affected code:
  - Modified: crates/fleety-cli/src/config_panel.rs
  - New: none
  - Removed: none
