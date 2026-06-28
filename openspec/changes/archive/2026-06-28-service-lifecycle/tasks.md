<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同 crate、無相依）。實機服務行為（真正 install/start 到 systemd/launchd/SCM、睡眠喚醒、關終端續跑）為環境相依，需手動驗證。 -->

## 1. 服務管理員抽象（fleety-tools）

- [x] 1.1 [P] 在 crates/fleety-tools 新增 service 模組：ServiceSpec { name, label, description, exec, args }、current_os、各平台 install/uninstall/start/stop/restart/enable/disable/status 的指令字串與服務檔內容「純函式」（systemd --user／launchd LaunchAgent／Windows SCM `sc`），以及實際執行的 wrapper——交付 "Background service is controlled by the CLI" 與 "Boot autostart can be toggled"（決策「服務管理員抽象與 CLI 動詞映射」「service.rs 重構：程式化 install/enable（保留純定義測試）」）。驗證:純函式單元測試涵蓋三平台×各動詞的指令/檔內容（含 fleetyd 與 fleety-server 兩個 ServiceSpec）;cargo test -p fleety-tools 全綠。
- [x] 1.2 在 service 模組加 pidfile 單例邏輯：寫入自身 pid、讀取、判斷 pid 死/活（無 unsafe 的最小相依或退化為警告）——交付 "The daemon is single-instance"（決策「單例：服務管理員 + pidfile defense-in-depth」）。驗證:單元測試「死 pid→可啟動、存活 pid→已執行、無檔→可啟動」。

## 2. fleetyd 韌性與控制

- [x] 2.1 [P] 在 crates/fleety-daemon/src/main.rs 把 run() 改成斷線重連迴圈：指數退避（base 1s／factor 2／cap 30s／±20% 抖動、成功歸零），斷線/睡醒自動重連、不阻止睡眠、Ctrl+C 與服務 Stop 乾淨退出——交付 "The daemon reconnects after disconnect or sleep"（決策「fleetyd 斷線重連與睡眠友善」）。驗證:退避序列計算純函式單元測試（取自 spec Example 表）+ 以可注入連線函式模擬連續失敗→驗證重試且退避遞增、成功歸零。
- [x] 2.2 fleetyd 子命令 start/stop/restart/enable/disable/status（用 service 模組）、install/uninstall 改為程式化執行（取代只寫檔+印指令）、run-service 服務模式進入點、啟動時寫 pidfile/退出時清除——交付 "Background service is controlled by the CLI"。驗證:cargo build -p fleety-daemon 綠;子命令 → 動詞分派的可測邏輯單元測試。

## 3. Windows SCM 服務

- [x] 3.1 加 windows-service 相依（target cfg windows，fleety-daemon 與 fleety-server）、SCM 服務進入點與控制 handler（Stop→graceful shutdown、回報 Running）、Windows 映射用 `sc`（含 `start= auto` 開機/未登入即跑，取代 Task Scheduler）、install 偵測非 admin 時回可行動錯誤；headless（session 0、無互動桌面）時桌面類工具（computer_*／可視瀏覽器）優雅退回可行動錯誤、不卡死——交付 "Windows runs as a real service"（決策「Windows 用真正的 SCM 服務（windows-service）」，含「未登入也能跑」與 session 0 桌面工具退回）。驗證:cargo build（cfg windows）綠;admin 偵測與錯誤訊息、headless 桌面工具退回的純邏輯單元測試。

## 4. fleety-server 服務化

- [x] 4.1 在 crates/fleety-server 加服務子命令 install/uninstall/start/stop/restart/enable/disable/status（用同一 service 模組與自己的 ServiceSpec）、install 預設 enable 開機自啟（可 disable）、up（install+enable+start）與 down（stop）便利命令、run-service 進入點、無子命令仍前景跑——交付 "Server autostarts by default and offers up/down"（決策「fleety-server 服務化與 compose 式 up/down」）。驗證:cargo build -p fleety-server 綠;up/down 組合與「安裝預設自啟」邏輯單元測試。

## 5. restart 延到閒置 + 自更新協調

- [x] 5.1 實作「延到閒置再重啟」：記下 pending restart（reason/force/截止），只在服務閒置（fleety-server 無 in-flight turn／fleetyd 無進行中 on-device 工具）才真正 restart；有 deferral 截止上限（約 300s）與冷卻（約 30s）；`force`（手動 restart）立即重啟——交付 "Restart waits for in-flight work"（決策「restart 延到閒置再重啟（graceful restart，借鏡 openclaw）」）。驗證:以可注入「是否閒置」與時鐘的純邏輯單元測試（忙→延遲、閒置→執行、force→立即、達截止→重啟）。
- [x] 5.2 update 與 poll_updates 在更新後**經 5.1 的延到閒置機制**重啟服務以套用新版（自更新不打斷進行中工作）；Windows 以「舊 exe 改名挪移、新 exe 寫回原路徑」避開執行中鎖定，再重啟服務——交付 "Self-update restarts the service"（決策「自更新與服務重啟協調」）。驗證:swap+（延到閒置）restart 的路徑/流程純邏輯單元測試;cargo build -p fleety-daemon 綠。

## 6. 文件

- [x] 6.1 docs/env.md（若新增 env）、安裝與服務說明、CLI 動詞語意（start/stop/restart 即時、restart 預設延到閒置、force 立即；enable/disable=開機自啟；up/down；Windows 需一次 admin、未登入也跑、session 0 桌面工具限制）——交付:文件與行為一致。驗證:內容審查。

## 7. 整體驗收

- [x] 7.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;並記錄實機服務行為需手動驗證的項目。
