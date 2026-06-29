## 1. 取消旗標與檢查點

- [x] 1.1 讓 `drive_to_goal`(crates/fleety-server/src/conn.rs)接受 `cancel: &AtomicBool`(標準庫、零依賴),在「每個 goal 回合之間」檢查;已取消則不起新回合、乾淨回傳已完成步數(非錯誤,並送出終端 Assistant + Done),交付 "A running turn can be cancelled at safe checkpoints";對應設計「輕量取消旗標,只在檢查點檢查」。先寫失敗測試(MockProvider 腳本):只腳本一回合 + 預設取消旗標 → drive_to_goal 不再迴圈/耗盡 provider、回 Ok 並送 Done;旗標恆 false → 行為與現況相同(既有測試)。
- [x] 1.2 因為取消發生在「回合之間」(從不中斷回合中途),已完成的回合照常持久化到對話歷史,後續處理新訊息的 follow-up 回合載入該歷史即見到先前工作,交付 "A cancelled run's work is preserved for the next turn";對應設計「被取消回合靠既有 journal 保存」。驗證:cancel 測試中先前回合的 Assistant 已送出/持久化;follow-up 回合在同一對話上跑(程式碼審查 + cancel 測試涵蓋)。

## 2. serve 並行讀取

- [x] 2.1 在 crates/fleety-server/src/conn.rs 以 `tokio::pin!` 的回合 future + `select!` 同時 poll「回合完成」與「下一則 inbound」(full-access 用 AutoApprove gate,讓 inbound 不被 approval 借用;require-approval 維持既有循序路徑);回合進行中收到的新訊息進入單一待處理槽,交付 "New messages are read while a turn is running";對應設計「回合改背景 task,serve 主迴圈並行讀 inbound」。驗證:既有對話/路由整合測試全綠;新訊息在回合 future 完成前被讀到並交給 triage(程式碼審查 + cancel 測試涵蓋停止語意)。

## 3. triage 與行動

- [x] 3.1 在 crates/fleety-server/src/triage.rs 實作純函式 `parse_triage(model_text) -> TriageAction`(interrupt_now / queue_after / ignore,無法解析 → queue_after 預設)與一次輕量模型呼叫的 `triage(new_msg, turn_summary, provider)`(MVP 用主 provider;路由到 cheap tier 列為後續),交付 "A triage decides how to handle a mid-turn message";對應設計「cheap triage:直呼便宜 tier 分類,輸出三選一」。先寫失敗測試:`parse_triage` 對代表三種決策的輸出與無法解析(→ queue_after)逐列驗證。
- [x] 3.2 在 serve 依 triage 決策行動:interrupt_now → 設取消旗標 + 把新訊息排成下一回合(其 context 含被中斷工作);queue_after → 當前回合結束後再跑;ignore → 不跑;三者皆回使用者一則 ack;triage 失敗預設 queue_after,交付 "A triage decides how to handle a mid-turn message" 的行動面;對應設計「依 triage 行動」。驗證:整合測試 interrupt_now 會在檢查點停並開新回合、queue_after 等完成再跑;既有單則對話回歸不變。
