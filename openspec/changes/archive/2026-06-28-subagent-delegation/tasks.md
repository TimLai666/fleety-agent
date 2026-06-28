<!-- 每項都含「交付行為 + 驗證目標」。tdd:true → 先寫失敗測試再實作。
     檔案路徑只是定位;任務本身講可觀察行為與驗證。 -->

## 1. Provider 與設定基礎(economy-model-tier)

- [x] 1.1 [P] 把 fleety-server 啟動時內聯的 provider 建構抽成可重用函式 build_provider(prefix),並用它從 FLEETY_CHEAP_MODEL_* 建第二 provider —— 交付 "Optional second economy provider":BASE_URL+MODEL 皆設才另建,provider 實作/model 可與主模型不同。驗證:單元測試以注入式 env 斷言「皆設→建出獨立 cheap provider」「主模型路徑不受影響」。
- [x] 1.2 實作 tier 解析 main/cheap 與未設回退 —— 交付 "Tier resolution and fallback":main→主 provider、cheap→cheap provider;cheap 未設時 cheap 別名為主 provider(Arc clone),選 cheap 永不報錯。驗證:單元測試斷言「cheap 未設時 model=cheap 跑在主 provider 且不 error」。

## 2. 委派核心(subagent-delegation)

- [x] 2.1 [P] 把建 ToolRegistry 的程式抽成可參數化建構器 include_orchestration:bool,子 agent registry = 父全集減 orchestration 工具 —— 交付 "Capability inheritance with one-level nesting":子 agent registry 不含 spawn_subagent/send_subagent_message/stop_subagent/subagent_status,但含 device_exec 與其餘所有工具;一層上限由工具缺席強制。驗證:單元測試斷言子 registry 的工具集差集(缺 orchestration、有 device_exec)。
- [x] 2.2 實作 SubagentRuntime 與 spawn/fork 執行器:用選定 tier 的 provider + 新的(spawn)或繼承父(fork)messages + 子 registry 巢狀呼叫 run_turn_streaming,前景回傳子 agent output —— 交付 "Spawn and fork subagents" 與 "Model tier selection"(含 fork 可換 tier)。驗證:單元測試斷言「spawn 初始 messages 不含父對話、fork 含父對話」與「model=cheap 走 cheap provider(注入式)」。
- [x] 2.3 實作 isolation none/worktree:worktree 時於子 agent 啟動前建獨立 git worktree、結束無變更則移除;非 git 工作區回可行動錯誤不靜默降級 —— 交付 "Isolation mode"。驗證:整合測試在 git 工作區斷言 worktree 建立+清除,非 git 工作區斷言回錯。

## 3. 非同步生命週期(subagent-lifecycle)

- [x] 3.1 實作任務註冊表與狀態機(Spawned/Running/Done/Failed/Stopped),前景 inline await、背景 tokio::spawn 並保留 handle —— 交付 "Asynchronous task registry and state machine"。驗證:單元測試斷言背景任務由 Running 轉 Done/Failed 且 registry 可查。
- [x] 3.2 實作 NotificationQueue:背景子 agent 終結時發使用者向通知 + 主動喚醒一個父 coordinator turn(seed 完成通知,不等使用者下一句,比照 Claude Code),以 task_id 去重(delivered 旗標)、近乎同時完成可批次成一次喚醒 —— 交付 "Background completion notification with de-duplication"。驗證:單元測試斷言「終結後 runtime 起一個父 turn 且恰 seed 一則該 task_id,之後不再投遞」。
- [x] 3.3 新增 send_subagent_message / stop_subagent / subagent_status 三工具:續談保留 messages、stop abort 背景 handle、status 回狀態與 output;未知 task_id 或對執行中 send 回可行動錯誤 —— 交付 "Continue and stop subagents"。驗證:單元測試斷言 stop→state=Stopped 且 handle 被 abort、未知 id 回錯。
- [x] 3.4 實作背景非互動 gate 與並行上限:full_access 放行、require_approval 背景僅 read(除非 allowed_tools 預授)、並行數可設(下限 1)、超限回可行動錯誤、子 agent 動作入父裝置 audit 並標 task_id —— 交付 "Non-interactive gate and concurrency limit"。驗證:單元測試斷言「超並行上限回錯」與「require_approval 背景子 agent 受限為 read」。

## 4. 接線與文件

- [x] 4.1 只在頂層 registry 註冊 orchestration 四工具(conn),於 server 啟動接好 SubagentRuntime + 主/cheap 兩 provider,並把完成通知以 ServerMsg 送回使用者 —— 交付:orchestration 工具僅頂層可見、子 agent 不可見(呼應 2.1 差集測試);背景完成有 UI 通知。驗證:單元測試斷言頂層 registry 含四工具、子 registry 不含;手動跑一次背景 spawn 看到通知。
- [x] 4.2 [P] 文件:docs/env.md 增 FLEETY_CHEAP_MODEL_*(對稱 FLEETY_MODEL_*)與並行上限變數;docs/tools.md 增 spawn_subagent/send_subagent_message/stop_subagent/subagent_status 四列(參數/risk/Runs on);prompts/protocol.md 增「何時委派」段。驗證:內容審查,工具列與 env 列齊全且與規格一致。

## 5. 整體驗證

- [x] 5.1 cargo fmt、cargo clippy --workspace -D warnings、cargo test 全綠(含上述新單元/整合測試);確認 agent-core 未被改動(維持無 Fleety 依賴)。驗證:三個指令零錯誤,git diff 確認 crates/agent-core 無變更。
