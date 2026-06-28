## Context

fleetyd 連上 server 後 hold，斷線即 break→退出（reconnect 是註解列的後續里程碑），睡醒不會接回；無單例、無背景化、CLI 僅 install/uninstall/update，install 只寫服務檔+印指令（crates/fleety-daemon/src/service.rs 有 systemd user/launchd/Task Scheduler 的純定義 service_def + 測試）。fleety-server 前景跑、關終端就死、無開機自啟。workspace `forbid(unsafe_code)` 使 Unix 正統 daemon 化(fork/setsid)不可用。使用者已拍板：全交給 OS 服務管理員；Windows 用真正的 SCM 服務（windows-service crate）；自更新需與服務協調。

## Goals / Non-Goals

**Goals:**
- 兩端皆可由 CLI install/uninstall/start/stop/restart/enable/disable/status 控制；無視窗、單例、關終端續跑、開機自啟可開關 —— 由 OS 服務管理員保證。
- fleetyd 斷線重連（指數退避），睡醒自動接回；不阻止睡眠。
- Windows 用 SCM 服務；安裝需一次 admin（偵測並提示）。
- 自更新後重啟服務套用新版。
- agent-core 不受影響、維持 host-free；平台指令映射為可單元測試的純函式。

**Non-Goals:**
- 不自寫 daemon 化（fork/setsid，受 forbid(unsafe) 限制）。
- 不阻止/喚醒裝置睡眠（不 caffeinate）；只在睡醒後重連。
- 不改 self-update 的下載/校驗邏輯，只加「更新後重啟服務」。
- 不改 agent loop／工具／協定。

## Decisions

### 服務管理員抽象與 CLI 動詞映射

在 fleety-tools 新增共用 `service` 模組（fleetyd 與 fleety-server 都依賴它）：一個 `ServiceSpec { name, label, description, exec, args }` + 依 `current_os()` 分派的管理員，提供 install/uninstall/start/stop/restart/enable/disable/status。各平台映射（指令字串由純函式產生、可測）：
- Linux systemd `--user`：install=寫 `~/.config/systemd/user/<name>.service` + `systemctl --user daemon-reload`；enable=`systemctl --user enable <name>`；disable=`disable`；start/stop/restart=`systemctl --user start|stop|restart <name>`；status=`is-active`/`is-enabled`。
- macOS launchd LaunchAgent：install=寫 `~/Library/LaunchAgents/<label>.plist` + `launchctl bootstrap gui/<uid>`；enable/disable=`launchctl enable|disable gui/<uid>/<label>`（plist RunAtLoad 控制開機）；start=`launchctl kickstart`；restart=`launchctl kickstart -k`；stop=`launchctl bootout`。
- Windows SCM：install=`sc create <name> binPath= "<exec> run-service" start= auto`（需 admin）；uninstall=`sc delete`；start/stop=`sc start|stop`；restart=stop+start；enable/disable=`sc config <name> start= auto|demand`；status=`sc query`。

**CLI 動詞語意（兩端一致）：** start/stop/restart＝現在跑不跑；enable/disable＝開機(登入)自啟開關（＝「開機自動啟動」）；install/uninstall＝註冊/移除服務；status＝回報「執行中?／開機自啟?」。

**替代方案：** 自管 pidfile+detached spawn——否決（forbid(unsafe) 下 Unix 脫離終端只能盡力，且要自維護一套生命週期，使用者已選服務管理員）。

### Windows 用真正的 SCM 服務（windows-service）

新增 `windows-service` crate（`[target.'cfg(windows)'.dependencies]`，fleety-daemon 與 fleety-server）。二進位被 SCM 以隱藏子命令 `run-service` 啟動時，進入服務模式：註冊 SCM 控制 handler（處理 Stop→觸發 graceful shutdown）、回報 Running、跑既有 daemon/server 邏輯。被 CLI 以 start/stop/… 呼叫時走上述 `sc` 映射。install 偵測非 admin 時回清楚的可行動錯誤（要求以系統管理員執行一次）。我們的 crate 仍 `#![forbid(unsafe_code)]`；windows-service 內部的 unsafe 屬該相依、非本 crate 程式碼。

**未登入也能跑（`start= auto` + LocalSystem）：** SCM 服務於開機啟動、不需任何使用者登入、登出亦不停 —— headless 工作（server 服務 WS、fleetyd 往外連、檔案/指令/MCP）正常。**但 Windows session 0 隔離**：服務跑在無桌面的 session 0，故**需要互動桌面的工具（`computer_*` 桌面控制/截圖、有視窗的瀏覽器）在「沒人登入」時無法運作**。處理：這類工具在 headless（無互動 session）時**優雅退回可行動錯誤、不卡死/不 crash**（沿用 never-crash），需要桌面時提示使用者登入。這是 Windows 本質限制，與 SCM/Task Scheduler 選擇無關（Task Scheduler onlogon 反而更糟：沒登入根本不跑）。

**替代方案：** 沿用 Task Scheduler（openclaw 在 Windows 即此路 + tray app）——否決：start/stop/restart 語意較弱、onlogon 需登入後才跑（不符「未登入也默默跑」），使用者選真正 SCM 服務。

### fleetyd 斷線重連與睡眠友善

`run()` 改成外層重連迴圈：連線→Hello→服務訊息迴圈；連線失敗或迴圈結束（斷線/睡眠導致的 read 結束）→以指數退避（base 1s、factor 2、cap 30s、±20% 抖動）等待後重連，成功後退避歸零。Ctrl+C 任一階段都乾淨關閉並退出（讓服務 stop 能即時生效）。不做任何阻止睡眠的事；睡醒時 OS 恢復網路後下一次重連即接回。token/裝置註冊沿用既有持久化（~/.fleety/fleetyd.token）。

### 單例：服務管理員 + pidfile defense-in-depth

主要單例由服務管理員保證（systemd/launchd/SCM 不會起第二份）。額外加 pidfile（`~/.fleety/<name>.pid`）：服務模式啟動時寫入自己的 pid；若檔存在且該 pid 仍存活則回「已在執行」並退出，避免使用者在服務之外又手動跑一份。退出時清除 pidfile。pid 存活檢查跨平台（Unix `kill(pid,0)` 經由現有安全 API／Windows OpenProcess 經 sysinfo 之類）—— 用不需 unsafe 的方式（如 sysinfo crate 或讀 /proc；Windows 用 windows-service/已有相依無法則以 `tasklist` 查），具體在 apply 時選最小相依；若無法可靠判活，退化為「檔在就警告但不強制」以維持 never-crash。

### fleety-server 服務化與 compose 式 up/down

fleety-server 加同套子命令（install/uninstall/start/stop/restart/enable/disable/status），用同一 fleety-tools `service` 模組、自己的 ServiceSpec（name="fleety-server"）。安裝預設啟用開機自啟（使用者要「自帶開機自啟」），可 `disable` 關掉。另提供 `up`（=install+enable+start，docker-compose-up-d 式：一行裝好、背景跑、關終端續跑）與 `down`（=stop）。直接 `fleety-server`（無子命令）仍前景跑，供開發。

### restart 延到閒置再重啟（graceful restart，借鏡 openclaw）

restart（含自更新觸發的重啟）**不可打斷進行中的工作**（agent 跑回合到一半被重啟會壞掉）。借鏡 openclaw `src/infra/restart.ts` 的作法：重啟請求不立即執行，而是**記下「restart 待辦」（reason/force/截止時間），等服務閒置（無進行中回合/工具）才真正重啟**；有截止上限（避免永遠不重啟，沿用 openclaw 的數量級：deferral 上限約 300s）與冷卻（約 30s）。`force` 旗標可略過延遲立即重啟（手動 `restart` 命令用）。實作上：一個程序內的「pending restart」狀態 + 「目前是否閒置」查詢（fleety-server 看是否有 in-flight turn；fleetyd 看是否正在跑 on-device 工具）；閒置時才呼叫服務管理員 restart（或自我乾淨退出讓 SCM/systemd 的 restart 政策接手）。手動 `restart` 預設 force（使用者明確要求即重啟）；自更新觸發預設延到閒置。

**替代方案：** 立即硬重啟——否決（打斷工作）；純靠 SIGUSR1 自重啟——可選的 Unix 優化，但統一用「pending + 閒置才重啟 + 服務管理員」較跨平台一致。

### 自更新與服務重啟協調

update 完成後**經上面「延到閒置」機制**重啟服務以套用新版（自更新預設延到閒置、不打斷工作）：閒置時呼叫服務管理員 restart。Windows 因執行中 exe 不可原地覆寫，swap 採「把舊 exe 改名挪開、新 exe 寫回原路徑」（執行中可改名），再 `sc stop`+`sc start`（或服務的 exit-restart）跑到新版。poll_updates 觸發的自動更新走同一路徑。fleetyd 既有 update.rs 的 swap 保留，僅補「延到閒置的重啟」與 Windows 改名挪移。

### service.rs 重構：程式化 install/enable（保留純定義測試）

把既有「只寫檔+印指令」升級為實際執行：install 寫檔後直接跑 daemon-reload/bootstrap（或對 Windows sc create），enable/start/... 實際呼叫管理員。保留 `service_def`/指令映射為純函式並保留/擴充其單元測試（不實際呼叫系統）。Windows 從 Task Scheduler 改為 SCM 服務定義。

## Implementation Contract

**Behavior:** `fleetyd install`（Unix 免 admin；Windows 需一次 admin）註冊服務並預設不阻止睡眠；`fleetyd enable` 設開機自啟、`disable` 取消；`start/stop/restart` 即時控制；`status` 回報執行與自啟狀態。服務以無視窗背景跑、關終端不死、單例。網路斷線或裝置睡眠後，fleetyd 以退避自動重連，睡醒即接回。`fleety-server up` 一行安裝+自啟+啟動（compose up -d 式），`down` 停止，安裝預設開機自啟且可 `disable`。自更新後服務自動重啟跑到新版。任何步驟失敗都回可行動訊息、不 panic。

**Interfaces / data shapes:**
- fleety-tools 新 `service` 模組：`ServiceSpec { name, label, description, exec, args }`；`fn current_os()`；管理員動詞 `install/uninstall/start/stop/restart/enable/disable/status -> Result<...>`；以及產生各平台指令字串的純函式（可測）。
- fleetyd 子命令：install/uninstall/start/stop/restart/enable/disable/status/update/run-service（run-service 為 SCM/服務模式進入點，隱藏）。
- fleety-server 子命令：install/uninstall/start/stop/restart/enable/disable/status/up/down/run-service；無子命令＝前景跑。
- 新依賴：windows-service（cfg windows）；pid 存活檢查所需的最小相依（apply 時定，偏好 sysinfo 或無相依方案）。
- pidfile：`~/.fleety/<name>.pid`。

**Failure modes:** 非 admin 安裝 Windows 服務→可行動錯誤（請以管理員執行）。管理員指令缺失/失敗→回該指令的 stderr 為訊息、不 panic。重連達不到 server→持續退避重試（服務內持續嘗試），不退出。pidfile 指向死 pid→視為可啟動並覆寫。Ctrl+C/服務 Stop→乾淨關閉、清 pidfile。自更新重啟失敗→記錄並保留舊版執行。

**Acceptance criteria:**
- 純函式單元測試：各 OS 的 install/enable/start/stop/restart/disable/status 產生的指令字串/檔內容正確（比照既有 service_def 測試，涵蓋三平台 + 兩個 ServiceSpec）。
- fleetyd 重連單元測試：退避序列（base/factor/cap/上限）計算正確；以可注入的「連線函式」模擬連續失敗→驗證會重試且退避遞增、成功後歸零（不需真正 socket）。
- pidfile 單元測試：寫入/讀取/死 pid 視為可啟動/存活 pid 視為已執行的判斷邏輯。
- restart-defer 單元測試：有進行中工作時 restart 被延遲、閒置時才執行；`force` 立即重啟；達截止上限即重啟（以可注入的「是否閒置」與時鐘驗證，不需真服務）。
- 內容審查：CLI 動詞語意、server 預設自啟可關、Windows admin 提示與「未登入也跑/session 0 桌面工具退回」、自更新延到閒置再重啟，文件齊備。
- 環境相依、需手動驗證（design 標明、不阻擋）：真正 install/start 到 systemd/launchd/SCM 的實機行為、睡眠/喚醒重連、關終端續跑。
- agent-core 不受影響（cargo tree 無 fleety-*）；cargo fmt + clippy --workspace -D warnings + test --workspace 全綠。

**Scope boundaries:**
- In：fleety-tools service 模組（管理員抽象＋指令映射）、fleetyd 重連＋子命令＋服務模式＋pidfile、fleety-server 服務化＋up/down、Windows SCM（windows-service）、restart 延到閒置（defer-until-idle）＋自更新協調、service.rs 重構、文件、純函式測試。
- Out：自寫 daemon 化、阻止/喚醒睡眠、self-update 下載/校驗改動、agent loop/工具/協定改動、agent-core 改動。

## Risks / Trade-offs

- [forbid(unsafe) 無法自寫 daemon 化] → 全交服務管理員（unsafe-free）；windows-service 的 unsafe 屬相依、本 crate 仍 forbid。
- [Windows 安裝服務需 admin] → install 偵測權限、給可行動提示；只需一次。
- [實機服務行為無法在 CI/此環境驗] → 把可測的「指令映射/退避/pidfile 邏輯」抽成純函式單元測試；實機 install/start 標為手動驗證。
- [自更新時服務持有 exe 鎖（Windows）] → 改名挪移 + 重啟服務；Unix 直接覆寫 + restart。
- [pid 存活判斷跨平台差異] → 用無 unsafe 的最小相依或退化為警告，維持 never-crash。
- [睡眠喚醒後 server 不可達] → 退避持續重連，不退出；不阻止睡眠。
- [新增 windows-service 相依] → 僅 cfg windows，其他平台不引入。
- [restart/自更新打斷進行中工作] → 延到閒置再重啟（pending + 閒置查詢 + 截止上限）；手動 restart 可 force。
- [Windows session 0：未登入時桌面工具不可用] → headless 時 `computer_*`/可視瀏覽器優雅退回可行動錯誤、不卡死；headless 工作（連線/檔案/MCP）照常。SCM 本來就是「未登入也跑」最佳解（Task Scheduler onlogon 更糟）。
