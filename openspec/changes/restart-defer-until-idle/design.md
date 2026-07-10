## Context

`crates/fleety-tools/src/restart.rs` 提供純函式 `decide(pending, idle, last_restart, now)` 與 `PendingRestart`：force → 立刻；過 deadline → 立刻；idle 且過 cooldown → 立刻；否則 wait。fleetyd 已在 process 內用它（`fleety-daemon/src/main.rs:334-362`、serve loop `:559-568`）——因為 fleetyd 的重啟觸發（auto-update 輪詢）本來就在同一個執行中的行程裡。

fleety-server 的情況不同：觸發重啟的是**另一個行程**（`fleety-server restart` CLI 呼叫，或 `fleety update` / daemon 收斂 shell 出去的 `fleety-server restart`，見 `fleety-cli/src/main.rs:303`、`fleety-daemon/src/main.rs:177`）。CLI 行程本身無法得知執行中 server 的 in-flight 狀態，也不該由它決定何時重啟。因此需要一條跨行程通道，讓外部呼叫「請求」執行中的 server 自行在 idle 時重啟。

## Goals / Non-Goals

Goals：
- 讓裸 `fleety-server restart`（含 update 觸發者）在有執行中 server 時，等 in-flight turn 清空（或過 deadline）才重啟。
- `--force` 與「server 沒在跑」退回即時 manager restart。
- 重用既有 `restart::decide`／`PendingRestart` 政策，不重造。

Non-Goals：見 proposal。特別是不引入網路控制面、不改 fleetyd 手動 restart。

## Decisions

### Cross-process restart request channel

外部 `fleety-server restart`（非 force）不直接呼叫 manager，而是在 runtime 目錄寫一個 restart-request marker 檔，然後立即返回並提示「已請求，idle 時重啟」。執行中的 server 由 watcher 消化此 marker。

選 marker 檔而非其他方案的理由：
- 跨平台一致（Windows SCM 無 POSIX 訊號；marker 檔在三平台一致，不必為 Windows 寫自訂 SCM control code）。
- 不新增對外網路面（避免 auth/安全面擴張）。
- 重用既有 `~/.fleety/<name>` runtime 目錄（`fleety_tools::service::pidfile_path` 已在此），marker 路徑由 `fleety_tools::service` 新增 `restart_request_path(name)` 提供。

marker 內容：寫入請求時間戳（供 watcher 計算 `PendingRestart` 的 deadline = 請求時間 + `DEFERRAL_CAP`）。marker 存在即代表「有一筆非 force 的 pending 重啟」，force 重啟不寫 marker（走即時路徑）。

替代方案（列於 Risks）：Unix 用 `SIGHUP` + Windows 用自訂 SCM control code。較符合 Unix 慣例、無輪詢，但雙平台雙路徑、Windows 端實作成本高，本次不採。

### In-flight turn accounting in fleety-server

新增 process-wide `AtomicUsize`（放在新檔 `crates/fleety-server/src/restart_watch.rs`），提供 RAII guard：進入 turn 執行前 `fetch_add(1)`，離開時 `fetch_sub(1)`。計數點：
- `conn.rs` `run_session` 的 turn 執行區段（WS 與 SSE+POST 共用此 loop，一次涵蓋兩種 transport），包住 recovery + 本次 turn。
- `scheduler.rs` 排程 turn 的 `run_turn` 區段（排程觸發的 turn 也不該被中斷）。

`idle` 定義：計數 == 0。guard 必須在所有 early-return / error 路徑都能正確遞減（用 Drop 實作，不手動配對）。

### Deferred-restart watcher and idle decision

server 啟動時（`run_server`）：
1. **清除啟動前殘留的 marker**：任何早於本行程啟動的 marker 都代表「它請求的那次重啟已經發生（就是這次啟動）」，直接刪除，避免開機即重啟的無窮迴圈。
2. spawn 一個 watcher task：以固定週期（例如每 2s）檢查 marker；存在時建構 `PendingRestart{ force:false, deadline:請求時間+DEFERRAL_CAP }`，呼叫 `restart::decide(p, inflight==0, last_restart, now)`；回傳 `RestartNow` 時刪除 marker、記錄本次 restart 時間（供 cooldown）、呼叫 `service::run_verb(spec, Verb::Restart)` 讓 manager 重啟自己（與 daemon `main.rs:564` 同模式：行程被 manager 停掉後重新拉起）。

watcher 是唯一會因 marker 而呼叫 manager restart 的地方；重複的 marker 寫入只會被消化一次。

### CLI restart verb: --force vs deferred default

`fleety-server/src/service.rs` 與 `main.rs` 的 arg 解析新增 `--force` 旗標：
- `restart`（無 force）：若 pidfile 顯示有存活的 server（`fleety_tools::service` 既有 `read_pid` + `pid_alive`），寫 marker 並印出「requested; will restart when idle」；否則（沒在跑）直接 `service::run_verb(Restart)`。
- `restart --force`：直接 `service::run_verb(Restart)`（即時，舊行為）。
`Action::Restart` 需帶 force 資訊（改 `Action` 或在 `run` 層處理旗標）。

### Routing update-triggered restarts through the deferred path

`fleety-cli/src/main.rs:303` 與 `fleety-daemon/src/main.rs:177` 維持呼叫裸 `fleety-server restart`（不加 `--force`），因而自動走 deferred 路徑；僅更新其使用者訊息／註解，把「An in-flight turn may be interrupted…」改為「will restart once the server is idle（過 deadline 才可能中斷，仍由 journal 續跑）」。

## Implementation Contract

- `fleety_tools::service::restart_request_path(name: &str) -> PathBuf`：回傳 `~/.fleety/<name>.restart-request`（與 `pidfile_path` 同目錄同慣例）。
- `restart_watch`（新模組）對外提供：
  - `turn_guard() -> impl Drop`：進入時 in-flight +1、drop 時 -1；`is_idle()` / 內部計數供 watcher 讀。
  - `clear_stale_request()`：`run_server` 啟動時呼叫，刪除既有 marker。
  - `spawn_watcher(spec)`：啟動輪詢 task，套用上述 decide 邏輯。
- 行為契約：
  - 有存活 server 時，非 force `restart` 不得在仍有 in-flight turn（且未過 deadline）時使行程終止；in-flight 清 0 後（或請求後滿 `DEFERRAL_CAP`）才重啟。
  - `--force` 與「無存活 server」立即重啟。
  - 排程觸發或 WS/SSE 的 turn 都計入 in-flight。
- 失敗模式：
  - marker 寫入失敗 → CLI 退回直接 manager restart 並提示（不可靜默不重啟）。
  - watcher 呼叫 manager restart 失敗 → 記 log、保留或刪除 marker 皆不得造成 busy-loop（比照 daemon `:565` 只記 warn）。
  - server 收到請求後在重啟前崩潰／被 `--force` 蓋過 → 新行程啟動時 `clear_stale_request` 清掉，不重播。
- 驗收：
  - 單元測試覆蓋 `restart_request_path` 形狀、in-flight guard 的 add/sub（含 error 路徑 drop）、watcher decide 分支（busy→wait、idle→now、force→即時不寫 marker、過 deadline→now）。
  - 行為驗證：對執行中 server 送非 force `restart`，觀察行程在 in-flight turn 結束前不終止；`--force` 立即終止重啟。

## Risks

- **訊號機制取捨需 arch 拍板**：marker 檔 vs（Unix `SIGHUP` + Windows SCM control code）。本設計採 marker 檔（跨平台單一路徑、無網路面），代價是 watcher 需輪詢且要處理 stale marker。若要求零輪詢／更即時，需改採訊號方案並補 Windows SCM control code——建議先確認再實作。
- **in-flight 計數遺漏點**：若有未經 `run_session` / scheduler 的 turn 執行路徑（例如未來新增 transport）未包 guard，會被誤判為 idle 而中斷。緩解：把 guard 綁在共用的 turn 執行入口，並在 review 時清點所有 `run_turn`/`drive_to_goal` 呼叫點。
- **stale marker 造成重啟迴圈**：靠 `clear_stale_request` 在啟動時清除；若清除失敗需確保 watcher 不會立刻消化到殘留 marker（可用 marker 時間戳 < 行程啟動時間則忽略作為雙保險）。
- **deadline 內仍會中斷**：過 `DEFERRAL_CAP` 後即使 busy 也重啟，仍靠 journal recovery。這是刻意取捨（避免無限拖延），文件需如實說明。
