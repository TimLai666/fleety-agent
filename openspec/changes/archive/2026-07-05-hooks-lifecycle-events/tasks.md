## 1. hooks_compat 事件擴增與執行 — 決策一:HookEvent 擴增，parse_hooks 涵蓋五事件；決策二:run_event_hooks——執行＋audit，只有 UserPromptSubmit 會阻擋；需求 Lifecycle-event hooks (UserPromptSubmit, Stop, SubagentStop)

- [x] 1.1 在 `crates/fleety-server/src/hooks_compat.rs` 的 `HookEvent` 加 `UserPromptSubmit`／`Stop`／`SubagentStop`，`as_str` 對映同名字串;`parse_hooks` 的事件迴圈改為涵蓋五個事件
- [x] 1.2 (紅) 寫測試 `parse_all_five_events`:一段含五事件（各一 command）的 settings，斷言 `parse_hooks` 解析出五筆、事件與 command 正確
- [x] 1.3 (綠) 讓 1.2 通過:擴 `HookEvent` 與 `parse_hooks` 事件清單（實現需求 Lifecycle-event hooks (UserPromptSubmit, Stop, SubagentStop) 的解析）
- [x] 1.4 (紅) 寫測試 `user_prompt_submit_blocks_on_nonzero`:假 runner + 假 audit，UserPromptSubmit hook 回 `Exited(1)` → `run_event_hooks` 回 `false`;Stop hook 回 `Exited(1)` → 回 `true`;兩者皆有 audit 紀錄
- [x] 1.5 (綠) 實作 `pub async fn run_event_hooks(event: HookEvent, hooks: &[HookEntry], runner: &Arc<dyn HookRunner>, audit: &Arc<dyn HookAudit>) -> bool`:對 `h.event == event` 逐一執行＋audit;僅 `UserPromptSubmit` 於 `Exited(code!=0)` 回 false，其餘一律 true，讓 1.4 通過

## 2. conn 服務迴圈觸發 — 決策三:conn 服務迴圈觸發 UserPromptSubmit（可阻擋）與 Stop；需求 Lifecycle-event hooks (UserPromptSubmit, Stop, SubagentStop)

- [x] 2.1 在 `crates/fleety-server/src/conn.rs` 定義 `async fn run_conversation_event_hooks(event, ctx: &HookContext, hub, pending, storage, device_id, conversation) -> bool`:ctx 內無該事件 hook 則回 true;否則以 `OriginHookRunner`／`HistoryHookAudit` 建 runner/audit 呼叫 `hooks_compat::run_event_hooks`
- [x] 2.2 在服務迴圈保留 `conv_hook_ctx: Option<Arc<HookContext>>`（綁定時 hooks 非空即設 `Some`），供事件 hooks 使用（與交給 subagent host 的同一 Arc）
- [x] 2.3 (綠) 使用者訊息 arm:在 dispatch（`let steps = …`）前呼叫 UserPromptSubmit;回 false 即 emit 一則「被 UserPromptSubmit hook 阻擋」的 Assistant＋Done 並 `continue`（不處理該回合）;dispatch 完成後呼叫 Stop（放行、僅 audit）
- [x] 2.4 (紅) 寫測試 `run_conversation_event_hooks_proceeds_when_no_such_event`（conn）:`HookContext` 只含一筆 PostToolUse，對 `UserPromptSubmit` 呼叫 `run_conversation_event_hooks` 回 true、且無 audit 產生（真實臨時 Storage 斷言 audit 空）

## 3. subagent SubagentStop — 決策四:SubagentStop 在 on_complete；需求 Lifecycle-event hooks (UserPromptSubmit, Stop, SubagentStop)

- [x] 3.1 (綠) 在 `crates/fleety-server/src/subagent.rs` 的 `FleetyHost::on_complete`，於喚醒回合前，若 `hook_ctx` 有值則以父對話 `context` 呼叫 `crate::conn::run_conversation_event_hooks(SubagentStop, …)`（放行、僅 audit）;巢狀 subagent 因共用同一 host 各自結束皆觸發

## 4. 驗證

- [x] 4.1 跑 `cargo test -p fleety-server` 全綠、`cargo clippy -p fleety-server`（`unwrap_used`/`expect_used` 無新違規）;修正殘留
