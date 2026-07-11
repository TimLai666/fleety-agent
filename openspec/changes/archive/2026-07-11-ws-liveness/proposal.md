## Why

WebSocket 是 Fleety 的主要傳輸，但目前完全沒有存活偵測：server 不發 ping、客戶端沒有 read deadline。連線被靜默斷開後（裝置睡眠、NAT idle 逾時、Wi-Fi 切換），server 的 Hub 仍把裝置當在線，`device_exec` 只能白燒完整的呼叫逾時才失敗；daemon 端的半開連線更會永遠掛著等 frame 而不觸發重連。SSE fallback 早已規範 keepalive 與 45 秒 half-open 偵測，主要傳輸 WS 反而是缺口。裝置在線狀態的正確性是 fleet 路由即時性與未來 presence 推定路線的地基。

## What Changes

- server 端 axum WebSocket adapter 改為 socket-owner task：週期送出 WS Ping（預設 20 秒），以「任何入站 frame」重設存活期限（預設 60 秒），逾期主動關閉連線並走既有的斷線清理路徑（Hub 與 device_tools 移除、writer task 結束）。
- 共用 client transport（fleety CLI 與 fleetyd 同用）的 WS 接收端加上 ping-adaptive read deadline：同一條連線看到第一個 server Ping 之後才啟用（預設 60 秒），逾期視為連線死亡回報結束，fleetyd 走既有指數退避重連、CLI 走既有 link-closed 處理。舊版 server 不發 ping 時不啟用，行為與現況完全相同，避免版本偏差造成誤判斷線。
- 新增兩個設定項：FLEETY_WS_PING_SECS（server 端 ping 週期）與 FLEETY_WS_TIMEOUT_SECS（雙端共用的存活期限），登錄進 config registry 並補上環境變數文件。
- 不改 fleety-protocol：Ping/Pong 是 WebSocket 幀層機制，ClientMsg/ServerMsg 訊息集與 protocol version 都不變；舊版 fleetyd 由 tungstenite 自動回 pong，不升級也能被新 server 正確偵測存活。

## Capabilities

### New Capabilities

- `ws-liveness`: WebSocket 傳輸的 keepalive 與半開連線偵測 — server 端週期 ping 加存活期限踢除，客戶端 ping-adaptive read deadline；是既有 SSE keepalive 規範在主要傳輸上的對應。

### Modified Capabilities

（無 — daemon-resilience 的重連需求不變，本變更只是新增一個觸發既有重連路徑的偵測來源；sse-transport-fallback 的 SSE 行為完全不動。）

## Impact

- Affected specs: 新增 ws-liveness（不修改既有 spec）
- Affected code:
  - Modified:
    - crates/fleety-server/src/http.rs
    - crates/fleety-tools/src/transport.rs
    - crates/fleety-tools/src/config.rs
    - docs/env.md
  - New: 無（不新增程式檔案）
  - Removed: 無
