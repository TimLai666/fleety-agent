## 1. In-flight turn accounting（fleety-server）

- [ ] 1.1 新增 `crates/fleety-server/src/restart_watch.rs`：process-wide `AtomicUsize` in-flight 計數與 RAII `turn_guard()`（Drop 時遞減）、`is_idle()`；單元測試驗證 add/sub 及 error/early-return 路徑仍正確歸零，對應「Restart waits for in-flight work」的 idle 定義。（design: In-flight turn accounting in fleety-server）
- [ ] 1.2 在 `crates/fleety-server/src/conn.rs` 的 `run_session` turn 執行區段（涵蓋 recovery + 本次 turn，WS 與 SSE+POST 共用）取用 `turn_guard()`；以 review 確認所有離開路徑都會 drop、無重複計數。
- [ ] 1.3 [P] 在 `crates/fleety-server/src/scheduler.rs` 排程 `run_turn` 區段取用 `turn_guard()`，確保排程觸發的 turn 也計入 in-flight（滿足 requirement 中「schedule-fired turns」）；review 排程 recovery 路徑一併涵蓋。

## 2. Cross-process restart request channel

- [ ] 2.1 [P] 在 `crates/fleety-tools/src/service.rs` 新增 `restart_request_path(name: &str) -> PathBuf`（`~/.fleety/<name>.restart-request`，與 `pidfile_path` 同慣例）並補單元測試驗證路徑形狀，作為「external invocation requests rather than hard-kills」的通道載體。
- [ ] 2.2 在 `restart_watch.rs` 加 marker 寫入 helper（含請求時間戳）與 `clear_stale_request()`（刪除啟動前殘留 marker，防開機重啟迴圈）；單元測試覆蓋寫入後可讀回、clear 後不存在。

## 3. Deferred-restart watcher and idle decision

- [ ] 3.1 在 `restart_watch.rs` 實作 `spawn_watcher(spec)`：週期檢查 marker，建構 `PendingRestart{force:false, deadline:請求時間+DEFERRAL_CAP}`，呼叫 `fleety_tools::restart::decide(p, is_idle(), last_restart, now)`，`RestartNow` 時刪 marker、記錄 restart 時間、`service::run_verb(spec, Verb::Restart)`；單元測試覆蓋 busy→wait、idle→now、過 deadline→now、cooldown 內→wait，實現「a busy server defers the restart」。
- [ ] 3.2 在 `crates/fleety-server/src/main.rs` 的 `run_server` 啟動時呼叫 `clear_stale_request()` 並 `spawn_watcher(spec)`；以 review 確認 watcher 是唯一因 marker 觸發 manager restart 的路徑。

## 4. CLI restart verb: --force vs deferred default

- [ ] 4.1 在 `crates/fleety-server/src/service.rs` 與 `main.rs` 的 arg 解析支援 `restart --force`；`Action::Restart` 帶 force 資訊，`run` 依旗標分流，滿足「forced restart is immediate」。
- [ ] 4.2 實作非 force `restart` 分流：以 `fleety_tools::service` 的 `read_pid`+`pid_alive` 判斷 server 是否存活——存活則寫 marker 並印「requested; will restart when idle」，未存活則直接 `run_verb(Restart)`（「no running server falls back to a manager restart」）；marker 寫入失敗則退回直接 manager restart 並提示。以 `fleety-server restart --help`/實測兩分支驗證。

## 5. Routing update-triggered restarts through the deferred path

- [ ] 5.1 [P] 在 `crates/fleety-cli/src/main.rs` `update_all`（約 :299-303）維持呼叫裸 `fleety-server restart`（不加 `--force`），並把「An in-flight turn may be interrupted…」訊息改為「will restart once the server is idle」，實現「Update-triggered server restart defers until idle」。
- [ ] 5.2 [P] 在 `crates/fleety-daemon/src/main.rs` 收斂路徑（約 :171-178）維持裸 `fleety-server restart`，更新註解與任何使用者訊息為 defer-until-idle 語意；review 確認未加 `--force`。

## 6. 文件與整體驗證

- [ ] 6.1 [P] 更新 `README.md`（及描述 server 重啟/中斷行為的相關 docs）把措辭改回 defer-until-idle：非 force restart 與 update 等 idle 才重啟、`--force` 或過 deadline 才可能中斷（仍由 journal 續跑）；以 `grep -rn "may be interrupted\|immediate" README.md docs` 確認無殘留矛盾描述。
- [ ] 6.2 全 workspace `cargo test -p fleety-server -p fleety-tools` 綠燈；`cargo clippy --workspace` 無 `unwrap_used`/`expect_used` 違規；行為驗證：對執行中 server 送非 force `restart` 觀察行程在 in-flight turn 結束前不終止、`--force` 立即重啟。
