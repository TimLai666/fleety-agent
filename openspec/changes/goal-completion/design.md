## Context

conn 每收到一則使用者訊息就跑一次 run_turn(內部 tool-loop 跑到模型輸出無 tool-call 的最終訊息),然後 emit Assistant+Done 結束。問題:模型可能「做一半就停下來問要不要繼續」。我們要一個內建、隨時在的 goal 機制 —— agent 自訂目標,迴圈盯著它做到完。Claude Code 是靠 prompt + 自管 todo + 可選 stop hook(軟);我們做明確訊號(硬)。

## Goals / Non-Goals

**Goals:**
- 永遠在的 goal 機制:5 個工具(set_goal/complete_step/goal_status/complete_goal/ask_user)+ drive-to-goal 迴圈 + 安全上限。
- 中間自動續做的回合靜默,只有 complete_goal/ask_user 出使用者回覆 + 口語。
- 通用 goal 狀態+工具放 agent-core;迴圈與發送/語音閘放 server。

**Non-Goals:**
- 不是可切換模式;不改 run_turn 單回合邏輯;不實作 TTS;不在 agent 沒呼叫 set_goal 時自動推斷目標。

## Decisions

1. **GoalState(agent-core,通用)。** { goal: Option<String>, steps: Vec<Step{text,done}>, terminal: Terminal }，Terminal = None | Complete{summary} | AskUser{question}。Arc<Mutex<GoalState>>,由工具改、由迴圈讀。

2. **五個工具(agent-core,只註冊在頂層)。** set_goal({goal, steps?}) 設目標+清單(可重設以更新計畫);complete_step({step}) 勾掉一步(以文字比對);goal_status() 回目標+各步狀態;complete_goal({summary?}) → terminal=Complete;ask_user({question}) → terminal=AskUser。subagent 的 child registry 不含這些(一層上限,與 orchestration 同模式 —— subagent 不該動父的目標)。

3. **premature 判定用明確訊號。** 一個 turn 結束時:goal 為 None → 正常單回合結束;terminal != None → 合法終止;goal 有、terminal 仍 None → 判定「過早停」→ 續做。不靠猜模型意圖。

4. **drive-to-goal 迴圈在 conn。** 每則使用者訊息:重設 GoalState;迴圈呼叫 drive_turn;每回合後讀 GoalState 決定停或續。續做時注入一句 continuation nudge(含目標 + 未完成的 steps + 「繼續;只有真的非問不可才呼叫 ask_user;做完才呼叫 complete_goal」)當成下一回合的 user 訊息。上限 FLEETY_GOAL_MAX_CONTINUES(預設一個合理值,下限 1)— 超過則停並回報達上限。

5. **發送與語音閘(副產物)。** drive_turn 加一個 emit_terminal 旗標:中間續做回合不 emit 最終 Assistant/Done(progress 仍以 AssistantDelta 串流),只有終止回合 emit 真正回覆。語音:口語 channel 只在終止回合產出(complete_goal/ask_user),所以「只在達成或必問時語音」自然成立。

6. **agent-core 不依賴 fleety。** goal.rs 只用 agent-core 既有型別(Tool/ToolRegistry/Value)。迴圈/發送/語音/cap 讀取屬 Fleety,放 conn/storage。

## Implementation Contract

**Behavior:** agent 收到需求 → 視需要 set_goal(可附 checklist);它做事,做完一步 complete_step;若中途真的需要使用者才能繼續 → ask_user(回覆+語音,等使用者);全部做完 → complete_goal(回覆+語音)。若它在目標未完成時就停 → 機制自動再催它繼續,直到 complete_goal/ask_user 或上限。沒 set_goal 的簡單請求行為照舊(單回合)。

**Interfaces(agent-core 公開):** GoalState、Terminal、register_goal_tools(&mut ToolRegistry, Arc<Mutex<GoalState>>)、GoalState 的查詢(is_active/terminal/take_terminal/pending_steps/nudge_text)供 conn 迴圈使用。五個工具名稱固定:set_goal/complete_step/goal_status/complete_goal/ask_user。

**Failure modes:** complete_step 比對不到的步 → 可行動錯誤(列出現有步);set_goal 空目標 → 錯誤;達 cap → 停止並在回覆說明「已達自動續做上限、目標可能未完成」;迴圈中任何 turn 失敗 → 沿用既有 run_turn 永不 crash(回錯誤訊息)。永不無限迴圈(cap 保證)。

**Acceptance criteria:**
- agent-core 單元測試:set_goal 後 is_active=true、terminal=None;complete_goal → terminal=Complete;ask_user → terminal=AskUser;complete_step 勾掉對應步、pending_steps 減少;未知步報錯。
- conn 行為(整合或單元):有 active goal 且未 complete → 迴圈再跑一次(以可注入的 turn 計數驗證);complete_goal/ask_user → 只跑該回合就停;達 cap → 停。
- 中間回合不 emit 終止 Assistant/Done(以 out 接收端斷言只有最後一回合有終止幀)。
- agent-core 仍不依賴任何 fleety crate(cargo tree)。
- cargo fmt + clippy --workspace -D warnings + test --workspace 全綠。

**Scope boundaries:** In:agent-core goal.rs(狀態+5 工具+helpers)、conn 的 goal 迴圈與發送/語音閘與 per-message 重設、cap env、docs(env/tools)、prompts(protocol/rules 教 agent 用 goal 並只在終止時說話)、測試。Out:TTS 實作、可切換模式、自動推斷目標、run_turn 改動、其他 crate。
