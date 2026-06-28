<!-- 每項含交付行為 + 驗證目標。tdd:true → 核心測試先寫。 -->

## 1. agent-core goal 狀態與工具

- [x] 1.1 在 crates/agent-core/src/goal.rs 實作 GoalState、Terminal(None/Complete/AskUser)、五個工具(set_goal/complete_step/goal_status/complete_goal/ask_user)與 register_goal_tools(&mut ToolRegistry, Arc<Mutex<GoalState>>),並從 lib 匯出;附 helpers(is_active/take_terminal/pending_steps/nudge_text)—— 交付 "The agent self-sets a goal and an optional checklist" 與 "Goal tools are top-level only and the core stays host-free"。驗證:cargo build -p agent-core 綠;cargo tree -p agent-core 無 fleety-*。
- [x] 1.2 agent-core 單元測試 —— 交付驗收:set_goal 後 is_active=true、無 terminal;complete_goal→Terminal::Complete;ask_user→Terminal::AskUser;complete_step 勾掉對應步、pending_steps 變少;未知步報錯;set_goal 空目標報錯。驗證:cargo test -p agent-core goal:: 全綠。

## 2. conn 的 drive-to-goal 迴圈

- [x] 2.1 在 crates/fleety-server/src/conn.rs 為每則使用者訊息建/重設 Arc<Mutex<GoalState>>、把 goal 工具註冊在頂層 registry(與 orchestration 並列;subagent child registry 不含),並用迴圈包住 turn:每回合後讀 GoalState —— 無 active goal 或 terminal 即停;active 且非 terminal 即注入 continuation nudge(目標+未完成步)再跑一回合 —— 交付 "Drive to the goal until a terminal signal"。驗證:整合/單元測試斷言「有 active goal 未 complete → 多跑一回合;complete_goal/ask_user → 該回合後停」。
- [x] 2.2 加自動續做上限 FLEETY_GOAL_MAX_CONTINUES(下限 1),達上限即停並在回覆說明可能未完成 —— 交付 "Auto-continuation is bounded"。驗證:測試「持續不 complete 的情況下,續做次數達 cap 即停」。

## 3. 發送與語音閘

- [x] 3.1 給 conn 的 turn driver 一個 emit_terminal 旗標:中間續做回合不 emit 終止 Assistant/Done(progress 仍以 AssistantDelta 串流),只有終止回合 emit 真正回覆;口語 channel 同理只在終止回合產出 —— 交付 "Only the terminal turn replies, and speaks"。驗證:測試以 out 接收端斷言「多回合迴圈只有最後一回合有終止幀,不是每回合一次」。

## 4. 文件與提示

- [x] 4.1 docs/env.md 增 FLEETY_GOAL_MAX_CONTINUES;docs/tools.md 增五個 goal 工具一節;prompts/protocol.md 與 prompts/rules.md 教 agent「依需求自訂 goal 並驅動到完成,做完才 complete_goal、非問不可才 ask_user;口語只在達成或必問時說」—— 交付:工具與行為對 agent 可見且一致。驗證:內容審查,工具列/env 列與規格一致。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心 host-free —— 交付 "Goal tools are top-level only and the core stays host-free" 的關鍵驗收。驗證:cargo fmt、cargo clippy --workspace -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*。
