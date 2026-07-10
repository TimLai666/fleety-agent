## Context

排程執行流程：`scheduler::tick`（crates/fleety-server/src/scheduler.rs:32）取出 `due_schedules`，逐筆以 unattended policy 跑 `run_turn`，把 user prompt 與 assistant 回覆寫入 `schedule-<id>` 對話，最後 `schedules::mark_fired` 設 `last_run`。目前有三個缺口：

1. `mark_fired`（crates/fleety-server/src/schedules.rs:232）只寫 `last_run`，沒有任何 outcome，使用者看不到成功/失敗與摘要。
2. `tick` 迴圈用 `let outcome = run_turn(...).await?;`（scheduler.rs:107），單筆失敗會經 `?` 直接 return 出 `tick`，被 `spawn` 記成一行 `scheduler tick error (isolated)` warning，該筆的 `mark_fired` 不會執行 → 失敗排程被靜默重試、其餘 due 排程也被跳過。
3. 沒有主動通知機制：結果只躺在 `schedule-<id>` 對話裡，除非使用者主動 `fleety resume schedule-<id>` 才看得到。

連線建立點在 `handle_connection`（crates/fleety-server/src/conn.rs:600）：送出 `Welcome` 後、進入 `while let Some(msg) = inbound.next_client()` 訊息迴圈前，是投遞 proactive 訊息的自然位置；`emit(out, &ServerMsg::Assistant{…})` 是既有的 server 主動推訊管道。

## Goals / Non-Goals

Goals：讓每次排程執行都留下可查的 outcome；讓失敗不再靜默且不中斷其他排程；讓使用者下次連線就看到自上次以來的排程結果（失敗優先）。

Non-Goals：見 proposal 的 Non-Goals（多租戶排程歸屬、per-device 去重、失敗自動重試、client UI 呈現、外部通道）。

## Decisions

### Run outcome record and notification watermark

在排程 JSON 檔新增兩個欄位，皆為 additive、舊檔缺欄時以「無」處理：

- `last_outcome`: 物件 `{ "status": "ok" | "error", "summary": <string>, "ts": <u64 unix secs> }`。成功時 `summary` 取 assistant 最終輸出截斷（上限約 500 字元、以 char boundary 安全截斷）；失敗時 `summary` 取 `run_turn` 錯誤的 `report()` 文字截斷。`ts` 用該輪 `tick` 傳入的 `now`。
- `last_notified`: `u64` unix secs，代表已投遞給使用者的最新 outcome 時間。缺欄視為 0。

「未通知」判定：`last_outcome` 存在且 `last_outcome.ts > last_notified`。投遞後把 `last_notified` 設為 `last_outcome.ts`。

`last_run`（既有，fire 起始時間）維持語意不變，作為觸發排程（`due_schedules` / cron 計算）的依據；outcome 的 `ts` 與通知獨立於它。

### Per-schedule failure isolation

`tick` 迴圈中對每筆 due 排程，把 `run_turn` 的結果改為 match 而非 `?`：

- `Ok(outcome)`：如現行寫入 assistant 訊息，記 `last_outcome{status:"ok", summary:截斷(outcome.output), ts:now}`，`journal_end`，`mark_fired`（設 `last_run`）。
- `Err(e)`：寫入一則說明失敗的 assistant 訊息（內容含 `e.report()` 摘要，讓 `schedule-<id>` 對話留下失敗紀錄），記 `last_outcome{status:"error", summary:截斷(e.report()), ts:now}`，`journal_end`，`mark_fired`，`tracing::warn!` 一行，然後 `continue` 下一筆。

`tick` 整體仍回傳 `Ok(fired_count)`；只有「無法取得 due 清單 / registry 建置」這類前置錯誤才向上冒泡。outcome 的寫入採 best-effort：寫檔失敗只記 log，不讓一筆 outcome 寫入失敗中斷整輪。

失敗即 `mark_fired` 的取捨：`at:` 觸發器失敗後不再觸發、`every:`/cron 等下一個週期，避免無限重試；代價是暫時性失敗（如 provider 短暫不可用）不會自動重試，使用者需視 outcome 自行重建排程。此取捨列入 Risks。

### Proactive delivery on connect

在 `handle_connection` 送出 `Welcome` 之後、訊息迴圈之前，呼叫新函式 `deliver_pending_schedule_notifications(storage, out, device_id)`：

- 掃 `storage.schedules_dir()` 下每個 `*.json`，取 `last_outcome` 與 `last_notified`，篩出「未通知」者，依 `last_outcome.ts` 由舊到新排序。
- 每筆以 `emit(out, &ServerMsg::Assistant{ conversation_id: "schedule-<id>", text, seq:0, speech:None, attention:None })` 投遞。`text` 格式：成功 `Schedule <id> ran OK: <summary>`；失敗以顯著前綴 `⚠ Schedule <id> FAILED: <summary>`，並附一行提示 `Resume with: fleety resume schedule-<id>`。
- 投遞成功後把該筆 `last_notified` 設為 `last_outcome.ts`（沿用 `mark_fired` 的 read-modify-write 寫檔模式）。
- 全程 best-effort：任何單筆讀/寫/emit 失敗只記 log，不阻斷連線建立、不影響後續訊息迴圈。

conversation_id 帶 `schedule-<id>` 讓使用者可直接 resume 取完整記錄；即使 client 只顯示文字，失敗仍可見。

### Owner-scoped delivery

投遞前解析連線的 acting user：`storage.acting_for_device(device_id)`。僅當它為非 Guest、且等於 `storage.acting_for_device(SCHED_DEVICE)`（scheduler 虛擬裝置擁有者）時才投遞，否則整段跳過。這在 v0 單擁有者模型下等同「投遞給擁有者」，同時避免把排程結果洩漏給 Guest 或他人裝置。`ActingUser` 需可比較（`PartialEq`）；若既有型別未實作則於 identity 模組補上（純衍生、無行為變更）。

## Implementation Contract

- 資料形狀：排程 JSON 於既有欄位（`id`/`trigger`/`tz`/`prompt`/`mandate`/`allowed_tools`/`enabled`/`last_run`）外，additively 新增 `last_outcome`（如上）與 `last_notified`（u64）。缺欄一律安全預設（`last_outcome` 無 → 該排程不產生通知；`last_notified` 無 → 視為 0）。
- 行為：
  - 每次 `tick` 對每筆 due 排程，無論成功或失敗都寫入 `last_outcome` 並 `mark_fired`；`tick` 回傳成功處理的筆數，不因單筆執行失敗而提前 return。
  - `schedule_list` 回傳的每筆物件包含 `last_run`（若曾執行）與 `last_outcome`（若曾執行）。
  - 連線建立時，擁有者裝置會收到每筆「未通知」排程 outcome 各一則 `ServerMsg::Assistant`（`conversation_id="schedule-<id>"`），失敗有顯著標示；投遞後該筆不再重複投遞（`last_notified` 前進）。
- 失敗模式：schedules dir 不存在 / 單筆檔案讀取或 JSON 解析失敗 / 單筆 emit 失敗 → 皆 best-effort 略過該筆並記 log，不影響其他排程、不影響連線或 tick 整體。
- 驗收條件：
  - 一筆會失敗的排程與一筆會成功的排程同時 due 時，兩筆都被 `mark_fired`、兩筆都寫入對應 `status` 的 `last_outcome`，`tick` 回傳 2。
  - 失敗排程的 `at:` 觸發器在下一次 `tick` 不再 due。
  - 擁有者連線收到未通知 outcome、Guest 連線收不到；同一 outcome 不會在同一裝置重複收到第二次。
- 範圍邊界：不改觸發器解析、cron/tz 計算、mandate 執行、recovery（crash-interrupted）路徑；recovery 路徑（`recover_schedule_turn`）維持現行 journal 語意，不在本變更納入 outcome 記錄（其收尾同樣可補記 outcome，列為選配、不阻擋本變更）。

## Risks

- 暫時性失敗不自動重試：provider 短暫不可用會被記成 `error` outcome 且 `mark_fired`；使用者需依通知自行處理。屬刻意取捨（避免無限重試 log 洗版）。
- 多裝置通知去重：單一 `last_notified` 浮水印下，擁有者的多台裝置只有最先連線者收到；其餘靠 `schedule_list` 補足。若日後要 per-device，需把浮水印改為 per-device 結構。
- 排程歸屬假設：owner-scoped 投遞依賴 v0「排程屬 scheduler 裝置擁有者」的假設；未來若引入多使用者建立排程，需在建立時記錄擁有者並改用它做投遞比對。
- 通知投遞進 `schedule-<id>` 對話而非當前對話，client 若不主動呈現非當前對話訊息，使用者體感可能較弱；文字本身仍送達，且指向可 resume 的對話。