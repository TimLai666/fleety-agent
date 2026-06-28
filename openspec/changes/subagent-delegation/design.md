## Context

Fleety 是單一 agent 的 tool-loop:agent-core 的 run_turn / run_turn_streaming 吃 provider（&dyn ModelProvider）、tools（ToolRegistry）、messages、events、policy、gate，跑「呼叫工具→餵結果→再呼叫」直到收斂。fleety-server 啟動時從 FLEETY_MODEL_* 建一個 Arc<dyn ModelProvider>（未設則 EchoProvider），conn 每收到一則使用者訊息就組 messages =[system]+歷史，呼叫 run_turn_streaming，把結果以 ServerMsg 回送。背景任務已有先例（scheduler 觸發 prompt、gc 週期清掃）。

run_turn 是純函式、可巢狀,因此 subagent 的本質就是:用「選定 provider + 新的或繼承的 messages + 一份減去 orchestration 工具的 ToolRegistry」再跑一次 run_turn,把最終 output 回傳父 agent。

agent-core 不得依賴任何 Fleety crate。因此 orchestration 的執行器、註冊表、通知、provider 接線全部放 fleety-server;agent-core 維持不變（run_turn 已足夠）。

## Goals / Non-Goals

**Goals:**

- 主 agent 能派生子 agent,子 agent 在自己的 context 內推理與用工具,完成後回傳結果。
- spawn(乾淨 context+briefing)與 fork(繼承父 messages)兩模式;兩者皆可選 main/cheap tier,fork 亦可換 tier。
- 可選的便宜模型第二 provider,與主模型可不同 provider/model;未設則 cheap 回退主 provider。
- 全套非同步:背景執行、完成主動回報(去重)、對同一子 agent 續談、停止;含任務註冊表與狀態機。
- 子 agent 能力 = 主 agent 全集減 orchestration 工具(一層巢狀上限);仍能 device_exec 操作別台裝置。
- none / worktree 兩種隔離。

**Non-Goals:**

- 不做跨裝置 spawn(remote isolation):子 agent 執行器只在 server 同進程;要在別台做事走 device_exec。
- 不允許多層巢狀。
- 不接受任意 model 名稱(只 main/cheap)。
- 不改 model-provider 既有主模型行為;不改任何既有工具行為。
- 不做 GUI 監控面板、自動 routing、跨機分散式排程。

## Decisions

1. **執行器放 fleety-server,agent-core 不動。** 新增 fleety-server 的 subagent 模組,持有 SubagentRuntime;agent-core 既有 run_turn_streaming 直接被巢狀呼叫,維持「agent-core 無 Fleety 依賴」。

2. **ToolRegistry 分層建構。** 把現有建 registry 的程式抽成可參數化的建構器,參數 include_orchestration: bool。頂層 conn 建「含 orchestration」的 registry;SubagentRuntime 為每個子 agent 建「不含 orchestration」的 registry。子 agent 因此天然拿不到 spawn/send/stop → 一層巢狀上限由「工具缺席」強制,而非執行期檢查。其餘工具(device_exec、browser、computer-use、mcp、wiki、filesystem…)完全相同。

3. **Provider 建構抽成可重用函式 build_provider(prefix)。** 從 FLEETY_MODEL_* 與 FLEETY_CHEAP_MODEL_* 各建一個 Arc<dyn ModelProvider>。cheap 兩個必填 env(BASE_URL+MODEL)有設才另建;否則 cheap = main 的 Arc clone。SubagentRuntime 持有 { main, cheap } 兩者。tier 解析:main→main、cheap→cheap(未設時即 main)。

4. **fork 換 model 與 cache。** fork 繼承父 messages 的「內容」,其價值與是否換 model 無關;prompt cache 重用只有在 tier/provider 不變時才順帶發生(不同 provider 本就不可能共用)。故允許 fork 換 tier,且不把 cache 當 fork 的前提。

5. **任務註冊表與狀態機。** SubagentRuntime 持有 TaskId→SubagentTask 的並行映射。SubagentTask 含:state、mode、tier、messages(供 fork/續談保留)、join handle(背景)、最終 output、worktree 路徑(若有)。狀態:Spawned→Running→Done | Failed | Stopped。前景:inline await run_turn;背景:tokio::spawn,handle 存入表。

6. **通知與主動回報(比照 Claude Code 的主動重新喚醒)。** 新增 NotificationQueue。背景子 agent 完成/失敗時:(a) 立即發一則使用者向 ServerMsg(SubagentDone)告知;(b) **主動喚醒**一個父 coordinator turn —— 把完成通知當成新輸入,seed 進父的 messages 後跑一個正常 turn,讓 coordinator 立即綜整或再派工,不等使用者下一句。這就是 Claude Code 的 `<task-notification>` 重新喚醒模型。去重:以 task_id 為鍵帶 delivered 旗標,投遞一次即標記;近乎同時的多個完成**可批次成一次喚醒**。防 runaway 不靠「不喚醒」,而靠:被喚醒的是一個正常受限 turn(coordinator 自行判斷是否續跑)、並行有上限(見決策 9)、使用者可隨時中斷。

7. **背景子 agent 的審批閘。** 背景執行無法互動式審批。決策:背景子 agent 用非互動 gate。full_access(預設)→ 放行;require_approval → 背景子 agent 僅限 read 工具,除非 spawn 時以 allowed_tools 預先授權。前景子 agent 沿用父的 gate。所有子 agent 動作照常進稽核(掛在父裝置的 audit log,標注 subagent task_id)。**tier 不影響任何權限**:cheap 與 main 只差在 provider,policy / gate / audit 完全相同,便宜模型的子 agent 不會因為便宜而被放寬或收緊。

8. **隔離 worktree。** isolation=worktree 時,在子 agent 啟動前對工作區建一個 git worktree,把子 agent 的檔案根指向該 worktree;結束後若無變更則自動移除。需工作區為 git repo;若否,回報可行動錯誤並不靜默改用 none。isolation=none 則共用父工作區。

9. **限額。** 並行子 agent 數設上限(env FLEETY_SUBAGENT_MAX_CONCURRENT,預設一個合理小值,clamp 下限 1);超限的 spawn 回報可行動錯誤而非無聲排隊。每個子 agent 沿用 LoopConfig 的 max_steps 上限。

## Implementation Contract

**Behavior(對使用者/agent 可觀察):** 主 agent 透過新工具派生子 agent;前景模式回傳子 agent 的最終 output;背景模式立刻回 task_id 並讓主 agent 繼續,子 agent 完成時使用者收到通知且 runtime 主動喚醒一個父 coordinator turn 綜整結果(不等使用者下一句)。子 agent 能用除 spawn/send/stop 外的所有工具(含 device_exec 跨裝置)。

**Interfaces / data shapes(新增工具,皆為 Mutate 風險,僅在頂層 registry):**

- spawn_subagent —— 必填 prompt:string;選填 mode:"spawn"|"fork"(預設 "spawn")、model:"main"|"cheap"(預設 "main")、run_in_background:bool(預設 false)、isolation:"none"|"worktree"(預設 "none")、allowed_tools:string 陣列(選填,限縮工具)、name:string(選填,1~2 詞任務名)。回傳:前景 → { task_id, state:"done"|"failed", output };背景 → { task_id, state:"running" }。
- send_subagent_message —— 必填 task_id:string、prompt:string;對既存且非執行中的子 agent 續談,保留其 messages。回傳同 spawn 的對應模式。
- stop_subagent —— 必填 task_id:string;取消子 agent(背景則 abort handle)。回傳 { task_id, state:"stopped" }。
- subagent_status —— 必填 task_id:string;回傳 { task_id, state, name?, output? }(讓主 agent 主動查背景任務)。

**Provider/設定契約:** 新 env FLEETY_CHEAP_MODEL_BASE_URL / _MODEL / _KEY / _STREAM,語意對稱於 FLEETY_MODEL_*。BASE_URL+MODEL 皆設才建第二 provider;任一未設則 cheap=main。

**Failure modes:** 未知 task_id → 可行動錯誤;對執行中子 agent send → 拒絕並說明;worktree 隔離但工作區非 git → 可行動錯誤;超過並行上限 → 可行動錯誤;cheap tier 但未設 → 靜默使用 main(非錯誤);子 agent 內部錯誤 → 標 state=Failed 並把錯誤摘要當 output 回傳(永不讓父 turn crash)。

**Acceptance criteria:** 
- spawn 前景跑出子 agent 並回傳 output;子 agent 的 registry 不含 spawn/send/stop(以單元測試斷言工具集差集)。
- fork 模式子 agent 的初始 messages 含父對話;spawn 模式不含。
- model:"cheap" 在有設第二 provider 時走第二 provider;未設時走 main(以注入式 provider 測試斷言)。
- 背景 spawn 立即回 running;完成後 NotificationQueue 有且僅有一則該 task_id 的 notification(去重測試)。
- stop_subagent 後 state=Stopped 且背景 handle 被 abort。
- isolation=worktree 在 git 工作區建出獨立 worktree 並於無變更時清除;非 git 工作區回錯。

**Scope boundaries(in/out):** In:fleety-server 的 subagent 模組(工具+註冊表+runtime+通知)、provider 建構抽函式、registry 分層建構、cheap env、docs/env.md 與 docs/tools.md 與 prompts/protocol.md 文件、單元測試。Out:agent-core 任何改動(run_turn 已足夠)、remote 跨裝置 spawn、自動觸發 coordinator turn、多層巢狀、任意 model 名稱、GUI 面板。
