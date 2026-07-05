## 1. Protocol 與 agent-core 核心

- [x] 1.1 [P] 在 crates/fleety-protocol/src/lib.rs 新增 `ClientMsg::CancelTurn { conversation_id: Option<String> }` frame(design 決策一:wire protocol 用獨立的 CancelTurn frame,不重載 UserMessage),交付 spec「An explicit CancelTurn frame cancels the in-flight turn」的線路形狀:serde 序列化/反序列化往返一致、conversation_id 可省略。驗證:比照該檔既有 frame roundtrip 測試新增一則單元測試,cargo test -p fleety-protocol 全綠。
- [x] 1.2 [P] 在 crates/agent-core/src/agent.rs 為 `run_turn_streaming_cached` 新增 `cancel: Option<&AtomicBool>` 參數(design 決策二:agent-core 的取消旗標只加在 run_turn_streaming_cached 參數上,包裝函式傳 None;`run_turn`/`run_turn_streaming` 簽名不變),並依 design 決策三:checkpoint 位置在「每個 tool call 執行前」與「每輪模型呼叫前」,取消時以哨兵結果補齊未執行的 tool call 實作;旗標已設時未執行的 tool call 不跑、各補文字含 `cancelled by user before execution` 的哨兵 tool result、`TurnOutcome.cancelled = true`、不再呼叫模型。交付 spec「A running turn can be cancelled at safe checkpoints」的 per-tool-call 細化。驗證:新增單元測試——scripted provider 一輪回兩個 tool call,第一個執行後設旗標 → 第二個得哨兵結果、provider 只被呼叫一次、outcome.cancelled 為真;未設旗標路徑既有測試不修改全綠;cargo run -p fleety-eval -- run crates/fleety-eval/goldens 15/15 不修改而過。

## 2. Server 接線

- [x] 2.1 在 crates/fleety-server/src/conn.rs 把取消旗標從 AtomicBool 改為 CancelFlag(AtomicU8:None/Triage/Explicit;design 決策四:取消來源以 AtomicU8 區分,收尾文案不同),triage InterruptNow 與 client 斷線兩處寫入點對應調整;drive_to_goal 收尾文案按來源區分——Triage 維持現行「Stopped between steps to handle your new message.」,Explicit 為「Cancelled at your request — work completed so far is preserved.」。驗證:新增/擴充 conn.rs 單元測試覆蓋兩種收尾文案;既有 drive_to_goal 測試全綠。
- [x] 2.2 在 conn.rs 的 mid-turn select inbound 分支處理 CancelTurn(design 決策五:conn 的 mid-turn select 分支處理 CancelTurn 並立即回 ack):設 Explicit、立即 emit ack「cancelling — stopping at the next safe point (a running tool finishes first)」;閒置時收到 CancelTurn 靜默忽略;並把取消旗標傳入 drive_turn → run_turn_streaming_cached 接通 1.2 的 per-tool-call checkpoint。交付 spec「An explicit CancelTurn frame cancels the in-flight turn」的伺服器行為。驗證:ws 整合測試(比照 conn.rs 既有整合測試模式)——慢工具 turn 進行中送 CancelTurn → 先收 ack、後收含「Cancelled at your request」的收尾與 Done、已完成工具留有審計;閒置連線送 CancelTurn → 無任何輸出訊息。
- [x] 2.3 [P] 在 docs/env.md 的 FLEETY_POLICY(require_approval)段補充:該政策下 server 不讀 mid-turn frame,CancelTurn 於 gated turn 期間無效(對齊 spec 限制句)。驗證:內容審閱,與 spec「An explicit CancelTurn frame cancels the in-flight turn」的 require-approval 限制敘述一致。

## 3. TUI

- [x] 3.1 [P] 在 crates/fleety-cli/src/tui.rs 與 crates/fleety-cli/src/main.rs 實作 spec「The TUI offers a cancel gesture」(design 決策六:TUI 以 turn_in_flight 狀態決定 Esc 語意):App 新增 turn_in_flight(Send 時設、Assistant/Error/斷線時清)與 `Action::CancelTurn`;Esc 優先序=核可 modal(deny)> turn_in_flight(送 CancelTurn、狀態列顯示 cancelling…)> 離開;Ctrl+C 永遠離開;main.rs 對 Action::CancelTurn 送出 ClientMsg::CancelTurn。驗證:tui.rs 單元測試——in-flight 時 Esc → Action::CancelTurn 且不 quit、idle 時 Esc → Quit、modal 時 Esc → Deny(擴充既有 approval modal 測試);cargo test -p fleety-cli 全綠。

## 4. ACP

- [x] 4.1 [P] 在 crates/fleety-cli/src/acp.rs 實作 spec「ACP methods map to the fleety-server agent」的取消語意(design 決策七:ACP session/cancel 轉發 CancelTurn,進行中的 prompt 以 stopReason cancelled 結束):session/cancel 通知 → 對 server 連線送 CancelTurn 並標記該 session cancelled;進行中的 session/prompt 於 server 的取消收尾後以 `{"stopReason":"cancelled"}` 回應(把固定的 stop_reason 參數化為 end_turn / cancelled 二值)。驗證:acp.rs 單元測試(in-memory duplex)——session/cancel 後對 server 寫出 CancelTurn frame、隨後的 prompt 回應 stopReason=cancelled;既有 acp 測試全綠。

## 5. 收尾驗證

- [x] 5.1 全面回歸與文件:cargo test --workspace 全綠、cargo run -p fleety-eval -- run crates/fleety-eval/goldens 15/15;README.md 的 `fleety tui` 列與 TUI 輸入框提示補「Esc=cancel(進行中)/quit(閒置)」語意說明。驗證:兩個命令的輸出、README 內容審閱。
