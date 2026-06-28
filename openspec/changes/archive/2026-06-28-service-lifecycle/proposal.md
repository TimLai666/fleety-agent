## Why

fleetyd（客戶端 daemon）與 fleety-server 目前都不是「乖背景服務」。fleetyd 連上 server 後一斷線就退出（reconnect 是註解明列的後續里程碑），所以**裝置睡醒後不會自動接回**；它沒有單例、沒有無視窗背景化、CLI 只有 install/uninstall/update，而 install 只寫服務檔再印出要使用者自己跑的指令。fleety-server 則完全在前景跑，**關掉終端就死**、沒有開機自啟。使用者要的是：兩端都默默在背景跑、不影響睡眠但睡醒能續、單例、可用 CLI 開關/重啟/設開機自啟，server 安裝後像 `docker compose up -d` 一樣關終端仍續跑。

## What Changes

- **服務管理員抽象（跨平台）**：一套 ServiceManager 把 install/uninstall/start/stop/restart/enable/disable/status 映射到 Linux systemd `--user`、macOS launchd LaunchAgent、Windows SCM 服務。無視窗、單例、關終端續跑、開機自啟都由管理員保證（unsafe-free，避開 forbid(unsafe) 下的 fork/setsid）。
- **fleetyd 韌性**：run() 改成斷線重連迴圈（指數退避＋上限＋抖動），斷線/睡醒自動重連；保留 Ctrl+C 乾淨關閉；單例由服務管理員保證，另加 pidfile 作 defense-in-depth。仍不阻止裝置睡眠。
- **fleetyd CLI 控制**：新增 start/stop/restart/enable/disable/status（enable/disable＝開機/登入自啟開關），既有 install/uninstall 升級為程式化執行（不只印指令）。
- **fleety-server 服務化**：同套 install/uninstall/start/stop/restart/enable/disable/status，安裝預設啟用開機自啟（可 disable 關掉），並提供 up（install+enable+start）與 down 的 compose 式便利命令；前景直接跑仍可（dev）。
- **Windows = 真正的 Windows 服務（SCM）**：用 windows-service crate 提供服務進入點/控制 handler；被 SCM 啟動時跑服務模式、被 CLI 呼叫時走 sc 控制；install 需一次系統管理員權限（偵測並給清楚提示）。
- **自更新與服務協調**：update 後重啟服務套用新版；Windows 執行中 exe 改名挪移避鎖定（既有 update.rs 已做 swap，補「重啟服務」）。

## Non-Goals

（細節取捨見 design.md 的 Goals/Non-Goals。）

## Capabilities

### New Capabilities

- `service-lifecycle`: 跨平台服務管理員 + CLI 動詞（install/uninstall/start/stop/restart/enable/disable/status，及 server 的 up/down），把 fleetyd 與 fleety-server 變成無視窗、單例、關終端續跑、開機自啟可開關的背景服務；Windows 用真正的 SCM 服務；與自更新協調。
- `daemon-resilience`: fleetyd 斷線重連（指數退避），裝置睡醒自動接回；不阻止睡眠；pidfile 單例保護。

### Modified Capabilities

（無。沿用既有 self-update／device-enrollment 規格，不改其行為。）

## Impact

- 受影響 specs：新增 service-lifecycle、daemon-resilience。修改：無。
- 受影響程式：
  - 修改：crates/fleety-daemon/src/service.rs（程式化 install/enable/start…取代只寫檔+印指令）、crates/fleety-daemon/src/main.rs（reconnect 迴圈、子命令 start/stop/restart/enable/disable/status、服務模式進入點）、crates/fleety-daemon/src/update.rs 與 crates/fleety-daemon/src/poll_updates.rs（更新後重啟服務）、crates/fleety-server/src/main.rs（服務子命令與服務模式進入點）、docs/env.md、docs/spec-v0.md 或相關安裝文件
  - 新增：crates/fleety-server/src/service.rs（server 端服務整合，沿用 fleetyd 的管理員抽象或共用模組）；視設計可能在 fleety-tools 或新共用處放跨平台 ServiceManager
  - 新增依賴：windows-service（target cfg windows，fleety-daemon 與 fleety-server）
  - 移除：無
- 關鍵驗收：fleetyd 斷線/睡醒自動重連；兩端可由 CLI install/start/stop/restart/enable/disable/status 控制、無視窗、關終端續跑、開機自啟可開關；Windows 走 SCM 服務、install 偵測 admin；自更新後服務重啟套新版；agent-core 不受影響仍 host-free；平台指令映射純函式可單元測試；workspace fmt + clippy -D + test 全綠。
- 環境相依、需手動驗證：真正 install/start 到 systemd/launchd/SCM 的實機服務行為。
