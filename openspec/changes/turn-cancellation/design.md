## Context

伺服器已具備部分取消基礎(mid-turn-interruption 能力):full-access 政策下,turn 以背景 task 執行,連線迴圈同時讀 inbound;mid-turn 的新 UserMessage 經 LLM triage 可設定 drive_to_goal 的 cancel 旗標(AtomicBool),在「goal iteration 之間」的 checkpoint 停止,已完成工作保留。缺口:(1) 沒有顯式取消的 wire frame,使用者無法「純取消」;(2) checkpoint 粒度停在 goal iteration 之間,單一 turn 內連續多個 tool call 無法中停(原 spec 明列 per-tool-call checkpoint 為 follow-up);(3) TUI 無取消手勢;(4) ACP session/cancel 是空操作;(5) RequireApproval 政策下 gate 佔用 inbound,沒有 mid-turn 讀取路徑。

相關既有元件:drive_to_goal 與 mid-turn select 迴圈(crates/fleety-server/src/conn.rs);turn loop 三個變體 run_turn / run_turn_streaming / run_turn_streaming_cached(crates/agent-core/src/agent.rs);interrupted_tool_result 哨兵(agent-core,turn journal 恢復時為無結果的 ToolCall 補哨兵結果);ACP 橋接(crates/fleety-cli/src/acp.rs)。

## Goals / Non-Goals

**Goals:**

- 使用者可從 TUI(Esc)與 ACP 編輯器(session/cancel)顯式取消進行中的 turn,不需夾帶新訊息。
- 取消延遲上限從「整個 goal iteration」縮短到「單一 tool call 的執行時間」。
- 取消後已完成的工作保留、journal 一致(無孤兒 ToolCall)、回覆明確標示已取消。
- 既有行為零回歸:從不取消時,turn loop 與 triage 路徑行為不變;fleety-eval goldens 不需修改。

**Non-Goals:**

- 不硬中斷正在執行中的工具(維持既有 spec 語意;執行中的 run_command 等跑完才停)。
- RequireApproval 政策下的 mid-turn 取消(該政策下 gate 佔用 inbound 讀取,結構性議題另案;核可提示本身已提供 deny)。
- 排程(unattended)turn 的取消、以及 mid-turn 插話後排入的 follow-up turn 的可取消化(沿用現況 MVP)。
- 舊版 server 對新 CancelTurn frame 的相容性墊片(見 Risks)。

## Decisions

### 決策一:wire protocol 用獨立的 CancelTurn frame,不重載 UserMessage

ClientMsg 新增 `CancelTurn { conversation_id: Option<String> }`。取消是控制訊號不是內容:重載空白 UserMessage 或魔法字串會進 triage、進對話歷史,語意髒。conversation_id 目前僅供記錄(單一連線同時只有一個 in-flight turn),帶上是為了未來多對話併行時不用改 wire shape。

### 決策二:agent-core 的取消旗標只加在 run_turn_streaming_cached 參數上,包裝函式傳 None

`run_turn_streaming_cached` 新增參數 `cancel: Option<&std::sync::atomic::AtomicBool>`;`run_turn` 與 `run_turn_streaming` 維持原簽名、內部傳 `None`。理由:LoopConfig 加欄位會破壞所有 struct literal 建構點(fleety-eval、subagent 等);只動最底層函式,既有呼叫端(eval、subagent、workflow)零改動。agent-core 只依賴 std,不違反依賴規則。

### 決策三:checkpoint 位置在「每個 tool call 執行前」與「每輪模型呼叫前」,取消時以哨兵結果補齊未執行的 tool call

turn loop 內,模型回傳的 tool_calls 逐一執行前檢查旗標;旗標已設 → 該 tool call 與其後所有同批 tool call 不執行,各補一筆哨兵 tool result(沿用 interrupted_tool_result 的既有模式,內容標明「cancelled by user before execution」),然後結束本 turn,不再呼叫模型。每輪模型呼叫前也檢查一次。這保證 messages 內每個 ToolCall 都有對應 ToolResult(journal/compaction/恢復不變),且取消最多等一個正在執行的工具。TurnOutcome 新增 `cancelled: bool` 欄位(agent-core 內部建構,對外只讀)。

### 決策四:取消來源以 CancelFlag 區分,收尾文案不同

drive_to_goal 的旗標型別從裸 AtomicBool 改為 server 內的 CancelFlag。**實作精煉(取代原提案的單一 AtomicU8)**:CancelFlag 內含兩個 AtomicBool——`stop`(實際的「下個 checkpoint 停止」信號)與 `explicit`(僅記錄原因供收尾文案)。這樣 `stop` 可直接以 `Some(&flag.stop)` 交給 agent-core 的 `Option<&AtomicBool>`,不需要 AtomicU8→AtomicBool 的代理同步(單一 AtomicU8 會強迫 agent-core 認識三態原因、或維護易錯的代理鏡像)。`request_triage()` 設 stop;`request_explicit()` 設 explicit+stop。conn.rs 兩處既有 store 點(triage InterruptNow、client 斷線)改叫 `request_triage()`;新增的 CancelTurn 分支叫 `request_explicit()`。收尾文案:triage 維持現行「Stopped between steps to handle your new message.」;顯式取消為「Cancelled at your request — work completed so far is preserved.」;取消時若無部分回覆(turn 在產出任何文字前就停),文案獨立成句。

### 決策五:conn 的 mid-turn select 分支處理 CancelTurn 並立即回 ack

mid-turn select 的 inbound 分支對 `CancelTurn` 設 CancelFlag=顯式,並立即 emit 一則 Assistant ack「cancelling — stopping at the next safe point (a running tool finishes first)」,讓使用者按下後馬上有回饋。turn 之外(閒置時)收到 CancelTurn:回覆「nothing to cancel」級別的 no-op 訊息或靜默忽略——採**靜默忽略**(取消競態:使用者按取消時 turn 剛好結束,不該多冒一則訊息)。

### 決策六:TUI 以 turn_in_flight 狀態決定 Esc 語意

App 新增 turn_in_flight(Send 時設、Assistant/Error/斷線清)。Esc 優先序:核可 modal(=deny,現行)> turn_in_flight(=送 CancelTurn,狀態列顯示 cancelling…)> 離開。Ctrl+C 永遠離開。on_key 回傳新 Action::CancelTurn,由 main.rs 送 frame。

### 決策七:ACP session/cancel 轉發 CancelTurn,進行中的 prompt 以 stopReason cancelled 結束

session/cancel(通知,無回應)→ adapter 對 server 連線送 CancelTurn,並標記該 session 的 cancelled 旗標;進行中的 session/prompt 等到 server 的(已取消)turn 完成訊息後,以 `{"stopReason":"cancelled"}` 回應(取代固定的 end_turn)。編輯器停止鍵因此得到規範內的完整閉環。

## Implementation Contract

**行為**

- TUI:送出訊息後、回覆完成前按 Esc → 狀態列顯示 cancelling…,隨後收到一則以「Cancelled at your request」開頭(或含該標記)的回覆與 Done;已執行完的工具效果保留。閒置時按 Esc 離開(現行);核可 modal 中按 Esc 仍是 deny。
- ACP(Zed):按編輯器停止鍵 → session/prompt 回應 stopReason=cancelled;Fleety 端該 turn 以取消收尾。
- 取消延遲:最長為「當前正在執行的單一工具」的執行時間;同批未執行的 tool call 不再執行。
- 從不取消:所有現有測試與 goldens 行為不變。

**介面 / 資料形狀**

- `ClientMsg::CancelTurn { conversation_id: Option<String> }`(serde 慣例與其他 frame 一致)。
- `run_turn_streaming_cached(..., cancel: Option<&AtomicBool>)`;`TurnOutcome.cancelled: bool`。
- 哨兵 tool result 內容含固定可測字串 `cancelled by user before execution`。
- ACP prompt 回應 `stopReason` 值:`end_turn`(現行)/ `cancelled`(取消時)。

**失敗模式**

- 閒置時收到 CancelTurn:靜默忽略(競態下不多冒訊息)。
- RequireApproval 政策下 turn 進行中送 CancelTurn:gate 迴圈吞掉、無效果——文件(docs/env.md 的 policy 段)註明此限制。
- 舊 server 收到 CancelTurn:反序列化失敗走既有 unknown-frame 處理(記 log、連線不中斷);取消無效果但不傷害對話。
- client 在取消 ack 與收尾之間斷線:沿用既有斷線 → cancel 路徑,行為不變。

**驗收條件**

- agent-core 單元測試:scripted provider 一輪回兩個 tool call,第一個執行後設旗標 → 第二個不執行、得哨兵結果、`outcome.cancelled == true`、不再有第二次模型呼叫。
- conn.rs ws 整合測試(沿用既有測試模式):慢工具 turn 進行中送 CancelTurn → 先收 ack、後收含取消標記的收尾與 Done;審計含已執行工具的記錄。
- TUI 單元測試:turn_in_flight 時 Esc → Action::CancelTurn;閒置 Esc → Quit;modal Esc → Deny(既有測試擴充)。
- ACP 單元測試(in-memory duplex):session/cancel 後,對 server 寫出 CancelTurn frame,且 prompt 回應 stopReason=cancelled。
- `cargo test --workspace` 全綠、fleety-eval goldens 15/15 不修改而通過。

**範圍邊界**

- In scope:上述五個檔案群(protocol、agent-core turn loop、conn 接線、TUI、ACP)與對應測試、docs/env.md 的 policy 限制註記、README 命令表 TUI 快捷鍵說明。
- Out of scope:RequireApproval 下的取消、排程 turn 取消、follow-up turn 可取消化、硬中斷執行中工具、多對話併行取消路由。

## Risks / Trade-offs

- [舊 server + 新 CLI 的 CancelTurn 不被理解] → server 的 unknown-frame 處理不中斷連線,取消退化為無效果;README/env.md 不另做相容表,以 `fleety update` 的統一升級路徑為準。
- [取消 ack 與 turn 自然完成的競態] → ack 先發、收尾後到,兩則訊息順序固定(同一連線序列化 emit);閒置 CancelTurn 靜默忽略避免多冒訊息。
- [哨兵結果進入對話歷史可能影響後續模型行為] → 哨兵文案明確標示取消語境(模型可理解「使用者取消了」),與既有 interrupted_tool_result 模式一致,恢復路徑已驗證此類哨兵無害。
- [AtomicU8 取代 AtomicBool 的改動半徑] → 僅 conn.rs 內部型別,agent-core 介面維持 bool 語意;既有兩處 store 點同步改,由整合測試覆蓋。
