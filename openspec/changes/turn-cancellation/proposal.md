## Why

2026-07-05 產品體驗稽核確認:使用者一旦送出訊息就無法取消進行中的生成——TUI 沒有取消手勢、ACP 的 session/cancel 是空操作(編輯器按停止鍵毫無作用)。伺服器端其實已具備 goal-checkpoint 取消機制(mid-turn-interruption:drive_to_goal 的 cancel 旗標、triage 分流、取消後保留已完成工作),但沒有任何「顯式取消」的線路入口,唯一觸發途徑是送一則恰好被 LLM triage 判為 InterruptNow 的新訊息,不可靠也不即時。

## What Changes

- Wire protocol 新增 ClientMsg::CancelTurn frame:純取消,不夾帶新訊息,由 client 明確觸發。
- fleety-server 的 mid-turn 讀取迴圈(conn.rs 內 drive_to_goal 的 select 分支)處理 CancelTurn:設定既有 cancel 旗標、立即回覆一則確認訊息、取消收尾時回覆明確標示「已取消,已完成的工作保留」。
- agent-core 的 turn loop(agent.rs 的 run_turn / run_turn_streaming / run_turn_streaming_cached)接受可選的取消旗標,在每次 tool call 執行前檢查——把既有的「goal iteration 之間」checkpoint 細化到「tool call 之間」,取消延遲從整個 turn 縮短到單一工具的執行時間。正在執行中的工具仍不被硬中斷。
- TUI:等待回覆或 streaming 期間按 Esc 送出 CancelTurn(狀態列顯示 cancelling…);閒置時 Esc 維持離開。核可 modal 中的 Esc 仍是 deny,優先權不變。
- ACP:session/cancel 通知轉發 CancelTurn 給 server,進行中的 session/prompt 以 stopReason "cancelled" 結束,符合 ACP 規範與編輯器(Zed)預期。

## Capabilities

### New Capabilities

(無)

### Modified Capabilities

- `mid-turn-interruption`:新增「顯式取消」requirement(CancelTurn frame、確認回饋、取消收尾語意);細化既有 checkpoint requirement 的粒度(goal iteration 之間 → tool call 之間,原 spec 已把此列為 follow-up)。
- `acp-adapter`:session/cancel 從無操作改為「轉發取消 + 以 stopReason cancelled 結束進行中的 prompt」。

## Impact

- Affected specs: `mid-turn-interruption`(修改)、`acp-adapter`(修改)
- Affected code:
  - Modified: crates/fleety-protocol/src/lib.rs(新增 CancelTurn frame)
  - Modified: crates/agent-core/src/agent.rs(turn loop 的 per-tool-call 取消 checkpoint)
  - Modified: crates/fleety-server/src/conn.rs(CancelTurn 處理、cancel 旗標傳遞至 run_turn、取消收尾訊息)
  - Modified: crates/fleety-cli/src/tui.rs 與 crates/fleety-cli/src/main.rs(TUI Esc=取消的狀態機與送出)
  - Modified: crates/fleety-cli/src/acp.rs(session/cancel 轉發與 stopReason)
  - New: (無)
  - Removed: (無)
