## Why

手動 `fleety-server restart` 與 `fleety update` / 跨主機收斂觸發的 server 重啟目前都是外部 `sc`/`systemctl` 即時重啟，會硬砍掉正在進行的 turn（靠 journal recovery 續跑，不遺失但體驗中斷）。`crates/fleety-tools/src/restart.rs` 的 defer-until-idle 政策目前只有 fleetyd 自我更新輪詢路徑在用，fleety-server 完全沒接（findings #46、#58）。

## What Changes

- fleety-server 維護一個 process-wide 的 in-flight turn 計數（`conn.rs` 的 `run_session` turn 區段、`scheduler.rs` 的排程 turn 都納入）；`idle` 定義為計數為 0。
- 新增跨行程「請求重啟」通道：外部 `fleety-server restart`（非 `--force`）不再直接呼叫 service manager，而是請正在執行的 server「idle 時自行重啟」；server 內建 watcher 依 `restart::decide` 判斷，idle 或超過 deferral deadline 才真正呼叫 manager restart。
- `fleety-server restart --force` 保留即時（外部直接 manager restart）路徑；`fleety-server` 未在執行時，`restart` 退回直接 manager restart（沒有 in-flight 可等）。
- update 觸發的 server 重啟（`fleety-cli` 的 `update_all`、`fleety-daemon` 收斂到 server 版本）維持呼叫裸 `fleety-server restart`，因而自動走 defer-until-idle；同步把相關 CLI 訊息與註解改回「will restart when idle」語意。
- 文件（README 與相關 docs）改回描述 defer-until-idle 行為。

## Non-Goals

- 不改 fleetyd 的手動 `fleetyd restart`：它維持即時重啟。要讓它 defer 需要對 daemon 做同一套跨行程訊號機制，屬後續變更（本次僅動 server 側與 update 的 server 重啟路由）。
- 不新增任何對外網路 / HTTP 控制端點作為訊號通道（本次採 runtime 目錄下的 marker 檔，見 design）。
- 不改 journal-based recovery：它仍是 `--force` 或超過 deadline 而真的中斷 turn 時的安全網。
- 不改 `restart::DEFERRAL_CAP`（300s）與 `COOLDOWN`（30s）常數，也不改 `restart::decide` 的政策邏輯。
- 不改 fleetyd 自我更新輪詢既有的 in-process defer 路徑（維持現狀）。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- service-lifecycle: 把「Restart waits for in-flight work」需求收斂為 force 才即時、裸 restart 一律 defer，並明訂 fleety-server 以 in-flight turn 計數為 idle 訊號、外部 restart 對執行中 server 是「請求 idle 重啟」而非硬砍；新增「update 觸發的 server 重啟走 deferred 路徑」需求。

## Impact

- Affected specs: service-lifecycle
- Affected code:
  - Modified: crates/fleety-server/src/service.rs
  - Modified: crates/fleety-server/src/main.rs
  - Modified: crates/fleety-server/src/conn.rs
  - Modified: crates/fleety-server/src/scheduler.rs
  - Modified: crates/fleety-tools/src/service.rs
  - Modified: crates/fleety-cli/src/main.rs
  - Modified: crates/fleety-daemon/src/main.rs
  - Modified: README.md
  - New: crates/fleety-server/src/restart_watch.rs
