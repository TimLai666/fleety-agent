## 1. Outcome 資料模型與記錄（Record each run's outcome）

- [ ] 1.1 在 crates/fleety-server/src/schedules.rs 擴充 `mark_fired`（或新增姊妹函式 `record_outcome`）以在設定 `last_run` 之外，additively 寫入 `last_outcome{status,summary,ts}`；提供一個把字串安全截斷（char boundary、上限約 500 字元）的小工具。驗證：新增單元測試 `mark_fired_records_last_outcome` 斷言寫入後 JSON 檔含 `last_outcome.status`/`summary`/`ts` 且 `last_run` 仍存在。（design: Run outcome record and notification watermark）
- [ ] 1.2 在 crates/fleety-server/src/scheduler.rs 的 `tick` 成功路徑，改為呼叫 1.1 的記錄函式帶入 `status:"ok"` 與截斷後的 `outcome.output`（Record each run's outcome）。驗證：擴充既有 `tick_fires_due_at_schedule_once`，斷言 fire 後排程檔 `last_outcome.status=="ok"`。

## 2. Per-schedule 失敗隔離（Per-schedule failure isolation）

- [ ] 2.1 在 crates/fleety-server/src/scheduler.rs 的 `tick` due 迴圈把 `run_turn(...).await?` 改為 match：`Err(e)` 時寫入一則失敗 assistant 訊息、記 `last_outcome{status:"error", summary:截斷(e.report())}`、`journal_end`、`mark_fired`、`tracing::warn!` 後 `continue`；`tick` 仍回傳成功筆數，不因單筆失敗提前 return（Per-schedule failure isolation）。驗證：新增測試 `tick_isolates_failing_schedule`，用一個會讓 `run_turn` 失敗的 provider 與一個會成功的排程同時 due，斷言兩筆都被 `mark_fired`、各自 `last_outcome.status` 為 `error`/`ok`、`tick` 回傳 2、失敗排程的 `at:` 下一輪不再 due。

## 3. schedule_list 帶出 outcome（Surface last run outcome in schedule_list）

- [ ] 3.1 [P] 在 crates/fleety-server/src/schedules.rs 確認並固化 `schedule_list` 輸出包含 `last_run` 與 `last_outcome`（欄位隨記錄原樣流出即可，必要時補明確組裝）（Surface last run outcome in schedule_list）。驗證：新增測試 `schedule_list_surfaces_last_outcome`，先 `record_outcome` 再呼叫 `schedule_list`，斷言回傳的該排程物件含 `last_run` 與 `last_outcome.status`。

## 4. 連線時 proactive 投遞（Proactively notify the owner on next connect + Owner-scoped delivery）

- [ ] 4.1 在 crates/fleety-server/src/schedules.rs 新增 `pending_notifications(dir) -> Vec<(id, last_outcome)>`（篩 `last_outcome.ts > last_notified`、依 ts 排序）與 `mark_notified(dir, id, ts)`（read-modify-write 設 `last_notified`），沿用既有 id 路徑檢查（Proactive delivery on connect）。驗證：新增測試 `pending_and_mark_notified_roundtrip`，斷言未通知者被列出、`mark_notified` 後不再列出。
- [ ] 4.2 [P] 若 `crate::identity::ActingUser` 未實作 `PartialEq`，於 crates/fleety-server/src/identity.rs 補上 derive（Owner-scoped delivery）。驗證：新增或擴充測試斷言 `acting_for_device` 兩次解析同一擁有者相等、Guest 不等於具名使用者。
- [ ] 4.3 在 crates/fleety-server/src/conn.rs 新增 `deliver_pending_schedule_notifications(storage, out, device_id)`：先檢查 `acting_for_device(device_id)` 為非 Guest 且等於 `acting_for_device(SCHED_DEVICE)`，否則 return；再對 `pending_notifications` 每筆 `emit(ServerMsg::Assistant{conversation_id:"schedule-<id>", …})`（失敗以 `⚠ … FAILED` 標示並附 `fleety resume schedule-<id>`）並 `mark_notified`，全程 best-effort（Proactively notify the owner on next connect）。在 `handle_connection` 送出 `Welcome` 後、訊息迴圈前呼叫它。驗證：以既有 `QueueInbound` 測試骨架新增 `connect_delivers_unnotified_schedule_outcomes`，斷言擁有者連線收到含該 outcome 的 `ServerMsg::Assistant` 且二次連線不重送；新增 `guest_connection_gets_no_schedule_notifications` 斷言 Guest/他人連線收不到（Owner-scoped delivery）。

## 5. 整體驗證

- [ ] 5.1 跑 `cargo test -p fleety-server schedules:: scheduler:: conn::` 相關測試與 `cargo clippy -p fleety-server -- -D warnings`，確認新測試全綠、無 `unwrap_used`/`expect_used` 違規；人工審閱一次失敗通知文字，確認失敗在 client 端文字上顯著可辨。