## 1. 時序設定面

- [x] [P] 1.1 依設計「時序預設 ping 20 秒、期限 60 秒，登錄 config registry」：在 crates/fleety-tools/src/config.rs 登錄 FLEETY_WS_PING_SECS（Server scope，預設 20）與 FLEETY_WS_TIMEOUT_SECS（Shared scope，預設 60），各附說明與正整數驗證器；行為契約：CLI 的互動設定面能看到兩鍵、非正整數與非數字值被驗證器拒絕、程式端解析落回預設。驗證：config.rs 測試模組新增斷言（兩鍵存在、預設值、非法值拒絕），cargo test -p fleety-tools 綠。
- [x] [P] 1.2 docs/env.md 為兩個新鍵各補一列：預設值、用途、「期限至少為 ping 週期兩倍」的建議關係、以及中介設備吞 WS 控制幀時的緩解（調大 FLEETY_WS_TIMEOUT_SECS 或 FLEETY_FORCE_SSE=1 改走 SSE）。驗證：內容審閱，欄位與表格格式與既有列一致。

## 2. Server 端 liveness

- [x] 2.1 依設計「server 端以 socket-owner task 承載 liveness」重構 crates/fleety-server/src/http.rs 的 serve_ws：socket 改由單一 task 獨占，WS 的 ClientInbound / FrameWriter adapter 改為 mpsc 通道形（與 SSE adapter 同構），task 收束時 inbound 回 None 使 run_connection 走既有清理。行為契約：此步驟不改任何對外行為，純結構重構。驗證：既有 fleety-server 測試全綠（cargo test -p fleety-server），含 SSE 與 WS e2e。
- [x] 2.2 在 socket-owner task 實作規格「Server sends WebSocket keepalive pings and reclaims half-open connections」：依 FLEETY_WS_PING_SECS 週期送 WS Ping；依設計「任何入站幀都重設存活期限」以任何 inbound frame（text/pong/其他控制幀）重設期限；超過 FLEETY_WS_TIMEOUT_SECS 無任何入站幀即記一筆含裝置識別的 liveness timeout log 並關閉連線走既有清理；Ping 寫入失敗立即收束不等期限。驗證：新增測試（以短時序參數）——(a) 健康閒置的 tokio-tungstenite client 存活超過兩倍期限仍在 Hub 且可路由；(b) 完成 Hello 後停止輪詢 socket 的 client 在「期限＋一個 ping 週期」內被移出 Hub，之後對它 device_exec 立即回 not-connected 類錯誤。

## 3. Client 端 read deadline

- [x] [P] 3.1 在 crates/fleety-tools/src/transport.rs 的 WS 接收端實作規格「Clients detect half-open WebSocket links with a ping-adaptive read deadline」，依設計「客戶端 read deadline 採 ping-adaptive 啟用」：同一連線觀察到第一個 Ping 幀後才武裝 deadline（FLEETY_WS_TIMEOUT_SECS）；武裝後每次等待幀套逾時、任何幀重設；逾期回 None（與連線關閉同一形狀）。驗證：transport 測試——(c) 先 ping 後靜默的假 server 使 recv_text 在期限內回 None；(d) 從不 ping 的假 server 下，recv 掛等超過期限窗口仍不被誤判結束。
- [x] 3.2 確認呼叫端零改動即得正確行為：fleetyd 的 serve 迴圈收到 None 走既有 Outcome::Disconnected 退避重連、CLI 走既有 link-closed 處理。行為契約：本變更不需修改 crates/fleety-daemon 與 crates/fleety-cli；若實作中發現必須修改，視為設計偏差回報。驗證：全 workspace cargo test 綠，並人工檢視兩個呼叫端無 diff。

## 4. 相容性驗證與收尾

- [x] 4.1 驗證規格「Liveness uses WebSocket control frames without protocol changes」與設計「用 WS 幀層 Ping/Pong，不改 fleety-protocol」：crates/fleety-protocol 零 diff（git status 確認）；SSE 相關測試不變綠；新增舊 client 相容測試——以不含本變更邏輯的裸 tokio-tungstenite client（等同舊 fleetyd 的 WS 層）連上新 server 完成 Hello，閒置超過期限仍存活且可路由（幀層 auto-pong 生效）。
- [x] 4.2 依設計「接受忙碌 daemon 的已知互動，不改工具執行模型」補文件化：在 server liveness 期限判定處與 fleetyd 的 RunTool inline 執行處各留註解，說明「阻塞超過期限的工具會使連線被回收：該呼叫早已被 device_exec 逾時判定失敗、工具副作用照常完成、daemon 工具結束後自動重連」。驗證：內容審閱兩處註解與設計文一致。
- [x] 4.3 選做（release 前煙霧測試）：真實 server + fleetyd，以防火牆規則模擬靜默丟包，觀察 server 在期限內移除裝置、fleetyd 在期限內偵測並退避重連。驗證：人工核對雙端 log 時間差落在「期限＋一個 ping 週期」內。
