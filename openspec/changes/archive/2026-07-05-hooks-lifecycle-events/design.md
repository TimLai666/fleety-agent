## Context

hooks-compat 已有:純解析（`parse_hooks`）、tool-wrapper（`HookedTool`／`wrap_tools`，只處理 Pre/PostToolUse）、正式 runner（`OriginHookRunner`:origin 本機／跨裝置）、audit（`HistoryHookAudit`）、env 政策、以及跨主對話與 subagent 共用的 `HookContext` 與 `wrap_registry_with_hooks`。

缺的是三個非工具層事件。它們不綁工具，不進 tool-wrapper;而是在對話生命週期的定點觸發:UserPromptSubmit 在使用者訊息 dispatch 前、Stop 在該訊息處理完後、SubagentStop 在 subagent 結束時。conn 服務迴圈在使用者訊息 arm 已握有 `hub`／`pending`／`storage`／`conversation`;`FleetyHost::on_complete` 已握有同組依賴與 `hook_ctx`。

## Goals / Non-Goals

**Goals:**

- 解析並執行 UserPromptSubmit／Stop／SubagentStop hooks，audit 之。
- UserPromptSubmit 非零 exit → 阻擋該提示（不處理該回合），比照 PreToolUse 否決的安全語意。
- 沿用既有 runner／audit／env／HookContext，不重造。

**Non-Goals:**

- Stop／SubagentStop 的強制續跑（重入回合迴圈）。
- hook stdout 注入 context。
- SessionStart／PreCompact／Notification。

## Decisions

### 決策一:HookEvent 擴增，parse_hooks 涵蓋五事件

`HookEvent` 加 `UserPromptSubmit`／`Stop`／`SubagentStop`，`as_str` 對映同名字串。`parse_hooks` 的事件迴圈從兩個擴為五個。這三者在 settings.json 同樣是 `hooks.<Event>[] = { hooks: [{ type:"command", command }] }`;matcher 對非工具事件無意義，解析仍讀（預設 `*`）但執行時不做工具名比對。

### 決策二:run_event_hooks——執行＋audit，只有 UserPromptSubmit 會阻擋

新增 `pub async fn run_event_hooks(event, hooks: &[HookEntry], runner: &Arc<dyn HookRunner>, audit: &Arc<dyn HookAudit>) -> bool`（回傳 proceed）。對 `h.event == event` 的每個 hook 執行＋audit;若 `event == UserPromptSubmit` 且某 hook `Exited(code!=0)` 則 proceed=false。Stop／SubagentStop 一律 proceed=true。不進 tool-wrapper（`wrap_tools` 仍只挑 Pre/PostToolUse，天然排除這三者）。

### 決策三:conn 服務迴圈觸發 UserPromptSubmit（可阻擋）與 Stop

conn 加 `run_conversation_event_hooks(event, ctx, hub, pending, storage, device_id, conversation) -> bool`:ctx 內無該事件 hook 則直接 proceed;否則以既有 `OriginHookRunner`／`HistoryHookAudit` 建 runner/audit 呼叫 `run_event_hooks`。服務迴圈保留 `conv_hook_ctx: Option<Arc<HookContext>>`（綁定時設定）。使用者訊息 arm:在 dispatch（`let steps = …`）前跑 UserPromptSubmit，回 false 即 emit 一則「被 UserPromptSubmit hook 阻擋」的 Assistant＋Done、`continue` 跳過該回合（不處理、不 append 該使用者訊息）;該訊息 dispatch 完成後跑 Stop（放行、僅 audit）。

### 決策四:SubagentStop 在 on_complete

`FleetyHost::on_complete`（subagent 剛結束）於喚醒回合前，若 `hook_ctx` 有值則以父對話 `context` 跑 SubagentStop（放行、僅 audit）。巢狀 subagent 各自結束皆會觸發，因共用同一 host。

## Implementation Contract

**Behavior（可觀察）:**

- origin 設 UserPromptSubmit command 非零 exit;使用者送出提示時該回合不被處理，使用者收到「被 hook 阻擋」訊息，audit 有該 hook 紀錄。command 零 exit → 正常處理。
- origin 設 Stop command;每則使用者訊息處理完後該 command 執行一次、audit 一筆;無論 exit 為何都不影響已回覆內容。
- origin 設 SubagentStop command;每個 subagent 結束時執行一次、audit 一筆。
- 這三事件為空時，行為與現況完全相同（零額外成本）。

**介面:**

- hooks_compat:`HookEvent` 三新變體＋`as_str`;`parse_hooks` 五事件;`pub async fn run_event_hooks(...)->bool`。
- conn:`run_conversation_event_hooks(...)->bool`;服務迴圈 `conv_hook_ctx` 保留、UserPromptSubmit 阻擋、Stop 觸發。
- subagent:`on_complete` 觸發 SubagentStop。

**驗證目標（測試名／可觀察行為）:**

- `parse_all_five_events`（hooks_compat）:一段含五事件的 settings 解析出五筆、事件正確。
- `user_prompt_submit_blocks_on_nonzero`（hooks_compat）:假 runner + UserPromptSubmit 非零 → `run_event_hooks` 回 false;Stop 非零 → 回 true;兩者皆 audit。
- `run_conversation_event_hooks_proceeds_when_no_such_event`（conn）:ctx 只含 PostToolUse → UserPromptSubmit 事件回 true、無執行。

**In scope:** 三事件解析／執行／audit、UserPromptSubmit 阻擋、conn 與 on_complete 觸發點。
**Out of scope:** 強制續跑、stdout 注入、其他事件。

## Open Questions

- 被 UserPromptSubmit 阻擋的提示是否要落存為對話紀錄:首版不 append（未處理即不留存，比照被擋提示不進流程），僅回一則即時訊息。若日後要保留「被擋」軌跡可再加。
- Stop 是否也該在排程／喚醒回合觸發:首版只在使用者訊息路徑觸發 Stop（Stop 語意對應「回應使用者結束」）;排程回合不觸發，避免語意漂移。
