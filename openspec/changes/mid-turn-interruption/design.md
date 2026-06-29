## Context

server 的 serve 迴圈逐則處理:`while inbound.next_client()` → 取 turn_guard → `await drive_to_goal` → 釋放,期間新訊息卡在傳輸層。drive_to_goal/drive_turn(agent-core/src/agent.rs)無取消機制。回合已有 journal(crash recovery 用,reconstruct_messages / journal_events)。子代理有 subagent_stop(handle.abort)但那是背景 task,非使用者回合。WS 的 rx 與 SSE+POST 的 in_rx 都已是獨立 channel(收訊不靠 turn 推進)。約束:agent-core 不依賴任何 fleety crate(取消旗標需在 agent-core 內、用標準庫);forbid unsafe;never-crash;env 測試單執行緒。

本 MVP 經 /spectra-discuss 收斂:用「取消 + 開新回合 + 重用 journal」取代真 suspend/resume,把風險壓到最低。

## Goals / Non-Goals

**Goals:**

- 回合進行中能即時讀到新訊息並由 cheap triage 決定處置,而非一律卡到回合結束。
- 能在安全檢查點(工具之間 / 回合之間)取消當前回合;被取消的回合不遺失(journal)。
- interrupt 後新訊息成為新回合,agent 能看到被中斷工作而續做或轉向。
- 不動傳輸層、不依賴 fleety crate 於 agent-core、不中斷 mid-tool / mid-stream。

**Non-Goals(後續階段,見 Open Questions):**

- 真 suspend/resume 同一回合。
- mid-tool / mid-stream 取消。
- approval gate 等待中的插話處理。
- triage 升級為完整 subagent fork。
- 多則插話合併 / 佇列管理超過單一待處理槽。

## Decisions

### 回合改背景 task,serve 主迴圈並行讀 inbound

serve 把 drive_to_goal 放進 `tokio::spawn`(或 JoinHandle),主迴圈 `select!` { 回合 JoinHandle 完成, inbound.next_client() }。回合進行中收到的新訊息進入「待處理槽」交給 triage。turn_guard 仍確保同時間只有一個回合在跑。理由:WS/SSE 收訊本就獨立 channel,只需把「等回合完成」與「等新訊息」並行化,改動侷限在 serve 編排。

### 輕量取消旗標,只在檢查點檢查

在 agent-core 定義一個輕量取消旗標(`Arc<AtomicBool>` 包成具名型別,例如 `Cancel`),傳入 drive_to_goal / drive_turn;在「每個工具呼叫之前」與「每個 goal 回合之間」檢查,若已取消則乾淨地提早結束該回合(回傳目前已完成部分,不再起新步驟)。不檢查 mid-tool(副作用)與 mid-stream(半截輸出)。理由:標準庫實作、零依賴、不違反 agent-core 不依賴 fleety crate;粗檢查點避免工具回滾與半截串流的複雜度。

### 被取消回合靠既有 journal 保存

回合本就 journal 每步;取消後 journal 留著(不 journal_end)。新回合啟動時,既有的 recover/reconstruct 路徑會把被中斷的工作帶進 context(與 crash recovery 同機制)。理由:重用既有機制達成「不遺失 + 新回合可見」,不需新狀態儲存。

### cheap triage:直呼便宜 tier 分類,輸出三選一

收到插話時,直呼 cheap tier(ProviderTiers 既有)做一次輕量分類:輸入=新訊息文字 + 當前回合精簡狀態(目前 goal + 最近一步摘要),輸出 `{ action: interrupt_now | queue_after | ignore, reason }`。決策字串的解析(模型輸出 → 三選一 enum)做成純函式以利測試。理由:便宜、低延遲;非完整 subagent(列後續)。

### 依 triage 行動

interrupt_now → 設取消旗標(當前回合於下個檢查點停),把新訊息排成「下一個回合」並在當前回合結束後立刻跑(其 context 含被中斷工作);queue_after → 不取消,當前回合自然結束後跑新訊息;ignore → 不跑,只回一則 ack。三種都回使用者一則簡短 ack 說明處置。理由:涵蓋「立刻插入 / 等之後 / 忽略」三種使用者期待。

## Implementation Contract

**行為(Behavior):**

- 無插話時:行為與現況相同(回合照跑、結束後讀下一則)。
- 回合進行中收到新訊息:跑 triage。
  - interrupt_now:當前回合在下一個檢查點(工具前 / 回合間)停止,已完成步驟保留於 journal;隨即以新訊息開新回合,新回合 context 含被中斷工作。
  - queue_after:當前回合完成後處理新訊息;使用者先收到「已收到,稍後處理」ack。
  - ignore:不處理,回 ack 說明。
- 取消永遠發生在安全檢查點;mid-tool / mid-stream 不被打斷。
- 任何路徑都不 panic;triage 失敗 → 預設 queue_after(保守:不打斷正在進行的工作)。

**介面 / 資料形狀:**

- agent-core:具名取消旗標型別(`Cancel`,內含 `Arc<AtomicBool>`,`is_cancelled()` / `cancel()`);`drive_to_goal` / `drive_turn` 增加 `cancel: &Cancel` 參數(附加式),在工具呼叫前與 goal 回合間檢查。
- agent-core:`drive_*` 在偵測取消時回傳「已完成到目前」的結果(不視為錯誤)。
- fleety-server:triage 函式 `triage(new_msg, turn_summary, cheap_provider) -> TriageDecision`,以及純解析 `parse_triage(model_text) -> TriageDecision`(三選一 + 預設)。
- fleety-server conn.rs:serve 以背景 task 跑回合 + select 並行讀 + 待處理槽 + 依決策行動;turn_guard 仍序列化實際回合執行。

**失敗模式:**

- triage 模型呼叫失敗 / 輸出無法解析 → 預設 `queue_after`(不打斷),記 log。
- 取消旗標在無插話時恆為 false → drive_* 行為不變。
- 待處理槽同時多則:MVP 只保留最後一則(或最早一則,spec 明訂),其餘以 ack 告知稍後;多則合併列後續。

**驗收標準(Acceptance):**

- 單元測試(agent-core):drive_to_goal 在「回合間」設了取消旗標後,不再起新回合、乾淨結束並回已完成部分;未取消時行為不變(用 MockProvider 腳本)。
- 單元測試:工具呼叫前檢查取消 → 後續工具不執行。
- 單元測試(fleety-server):`parse_triage` 對代表 interrupt_now / queue_after / ignore 的模型輸出與無法解析(→ queue_after 預設)。
- 整合/手動:回合進行中送新訊息,interrupt_now 會在檢查點停並開新回合;queue_after 會等完成;既有單則對話行為回歸不變。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒可跑。

**範圍邊界:**

- In scope:並行讀取編排、取消旗標 + 檢查點、journal 重用、cheap triage 三選一 + 行動、ack。
- Out of scope:真 suspend/resume、mid-tool / mid-stream 取消、approval 等待中插話、triage 完整 subagent 化、多則插話合併佇列。

## Risks / Trade-offs

- [背景回合 task + 取消使並發路徑變多,易有競態] → turn_guard 仍序列化實際回合;取消旗標單向(只從 false→true);待處理槽單一,降低狀態空間。
- [粗檢查點:長工具仍會跑完才停] → MVP 接受(不中斷 mid-tool);使用者 ack 會說明「將於目前步驟後處理」。
- [triage 增加一次便宜模型呼叫延遲] → 只在「回合進行中收到訊息」時觸發;失敗預設 queue_after。
- [新回合看到 journal 的被中斷工作可能誤續做] → 由 agent 判斷;ack 與新回合提示說明被中斷狀態。
- [agent-core 取消旗標 API 變更觸及 drive_* 簽章與所有呼叫端] → 附加參數、機械式更新;未取消即等同現況。

## Migration Plan

- 附加式:drive_* 新增取消參數,傳入永不取消的旗標即等同現況(供既有呼叫端 / 測試)。serve 編排改並行讀但無插話時行為不變。
- 無資料遷移(journal 機制沿用)。
- 回滾:serve 不啟用並行讀 / triage(走原本逐則)即回到現況。

## Open Questions

- 真 suspend/resume 同一回合是否值得做(MVP 用取消+開新回合替代)。
- mid-tool / mid-stream 取消的安全做法(工具回滾、串流中止語意)。
- approval gate 等待中收到插話的處置(目前等待本身就在讀 inbound)。
- triage 升級為完整 subagent fork(更多上下文 / 工具)的時機。
- 多則插話的佇列與合併策略(MVP 僅單一待處理槽)。
