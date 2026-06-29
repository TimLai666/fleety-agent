## Why

當 fleety 回應/做事做到一半,使用者傳的新訊息目前**卡在傳輸層**直到整個回合跑完才被讀取(conn.rs 的 serve 迴圈 await drive_to_goal、turn_guard 持有整個回合)。訊息不會掉,但使用者**無法插話或打斷**——既沒有並行讀取,也沒有取消使用者回合的機制(只有 subagent_stop 能停背景 subagent),drive_to_goal/drive_turn 也無 cancellation。希望能在回合進行中即時處理插話:由一個便宜的 triage 判斷要立刻插入、排到之後、還是忽略。

## What Changes

- **並行讀取**:回合改成在背景 task 執行,serve 主迴圈以 select! 同時等「回合完成」與「收到新 inbound」。WS 的 rx 與 SSE 的 in_rx 都已是獨立 channel,只動 serve 編排,不動傳輸層。
- **檢查點取消**:drive_to_goal / drive_turn 接受一個輕量取消旗標(agent-core 內以原子布林實作,不引入新依賴、不依賴 fleety crate),只在「工具呼叫之間」與「goal 回合之間」檢查;不中斷 mid-tool、不中斷 mid-stream。被取消的回合靠既有 journal 保存,不遺失。
- **cheap triage**:回合進行中收到新訊息時,直呼便宜 tier 做分類(輸入=新訊息 + 當前回合精簡狀態),輸出 `{ interrupt_now | queue_after | ignore }`。
- **依結果行動**:interrupt_now → 設取消旗標,當前回合於下個檢查點停止,新訊息排成新回合(其 context 含被中斷工作,agent 自行續做或轉向);queue_after → 當前回合完成後再跑(回使用者 ack);ignore → 只 ack。
- 「恢復」= agent 看到 journal 中被中斷的工作後自行決定續做或 set_goal 轉向,**非**機器級 suspend/resume。

## Non-Goals

(本變更會建立 design.md,Non-Goals / 後續階段寫在 design 的 Goals/Non-Goals 與 Open Questions。)

## Capabilities

### New Capabilities

- `mid-turn-interruption`: 回合進行中即時處理新訊息——並行讀取(背景回合 task + select)、檢查點取消旗標(工具/回合之間,重用 journal 保存被取消回合)、cheap triage 三選一決策、以及依決策插入/排隊/忽略。MVP 不含真 suspend/resume。

### Modified Capabilities

(none)

## Impact

- Affected specs: mid-turn-interruption(新)
- Affected code:
  - Modified:
    - crates/agent-core/src/agent.rs(drive_to_goal / drive_turn 接受取消旗標,在工具之間 / 回合之間檢查並提早結束)
    - crates/fleety-server/src/conn.rs(serve 編排:背景回合 task + select 並行讀 inbound + triage + 依決策行動)
    - crates/fleety-server/src/providers.rs 或 subagent host(triage 用既有 cheap tier 解析,如需)
  - New:
    - crates/agent-core 內取消旗標型別(輕量原子旗標)
    - crates/fleety-server 內 triage 決策(直呼 cheap 模型 + 解析三選一)的模組/函式
