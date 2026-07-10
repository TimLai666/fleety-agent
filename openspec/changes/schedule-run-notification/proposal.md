## Why

排程在 unattended 下執行後，結果只寫進 `schedule-<id>` 對話、失敗只留在 server log，使用者無從得知排程跑了沒、跑出什麼；更糟的是 `tick` 裡 `run_turn(...).await?` 會讓單一排程失敗直接中斷整輪、且不呼叫 `mark_fired`，導致失敗的排程被靜默重試而使用者永遠看不到。

## What Changes

- scheduler 每次 run 收尾時，把 outcome（`status` 成功/失敗 + 一句摘要 + 時間戳）寫回排程檔的 `last_outcome`，成功與失敗都寫。
- 把 `tick` 的每筆排程執行改為 per-schedule 隔離：`run_turn` 回傳 `Err` 時記為 `error` outcome、正常收尾（journal_end + `mark_fired`）並繼續下一筆，不再中斷整輪、也不再無限重試。
- `schedule_list` 明確帶出 `last_run` 與 `last_outcome`，讓使用者查得到上次結果。
- 使用者下次連線時，把「自上次通知以來新完成、尚未通知」的排程 outcome（失敗特別標示）當作 proactive 訊息投遞到其裝置（重用既有 `ServerMsg::Assistant` 投遞），並推進每筆排程的通知浮水印 `last_notified`。
- 投遞限定給排程擁有者（與 scheduler 虛擬裝置解析出同一 acting user 的連線），Guest 或他人裝置不會收到。

## Non-Goals

- 不做多使用者「每個排程各自的擁有者」模型：沿用 v0 單擁有者假設，排程視為 scheduler 裝置擁有者所有。多租戶排程歸屬留待後續。
- 不做 per-device 通知去重：通知浮水印是每筆排程單一 `last_notified`，同一擁有者若有多台裝置，只有最先連線的那台會收到該次通知（其餘裝置仍可用 `schedule_list` / `fleety resume schedule-<id>` 查看）。
- 不做失敗自動重試或退避策略：失敗即記為終態 outcome 並 `mark_fired`（`every:` 等下個週期、`at:` 不再觸發）。
- 不改動 client 端如何在 UI 呈現非當前對話的 proactive 訊息，投遞契約止於 server 發出 `ServerMsg::Assistant`。
- 不新增 email/推播等外部通道，僅在既有 WebSocket 連線上投遞。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- scheduling: 新增「記錄每次 run 的 outcome」「per-schedule 失敗隔離」「schedule_list 帶出 outcome」「連線時 proactive 投遞未通知的排程結果」四項 requirement。

## Impact

- Affected specs: scheduling
- Affected code:
  - Modified: crates/fleety-server/src/schedules.rs
  - Modified: crates/fleety-server/src/scheduler.rs
  - Modified: crates/fleety-server/src/conn.rs