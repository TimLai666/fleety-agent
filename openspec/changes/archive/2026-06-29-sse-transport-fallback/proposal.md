## Why

裝置(daemon/CLI)與 server 之間只有單一傳輸:裸 TCP 上的 WebSocket。當 WS 連不到——尤其 WS upgrade 被公司 proxy 或防火牆擋掉、或某些環境只放行一般 HTTP——裝置就完全連不上,沒有任何退路。需要一條在「能跑一般 HTTPS、但擋 WS」環境下仍可用的備援傳輸。

## What Changes

- 新增 **SSE(下行 server→client)+ HTTP POST(上行 client→server)** 備援傳輸。WS 仍是預設;SSE+POST 只在 WS 連不到時啟用。SSE 單向,故上行另走 POST,兩條 HTTP 請求以 session id 關聯。
- server 端在最前面放 **axum** router,同一個 listener/port 同時處理:WS upgrade、`GET /sse`(`text/event-stream` 下行串流)、`POST /send`(上行訊息)。現況的裸 TCP + tokio_tungstenite accept 路徑改由 axum 接手。**BREAKING(內部)**:server 連線入口從 raw-TCP-accept 改為 axum,需新增 axum 依賴。
- 抽象傳輸:server 的連線服務迴圈改成吃一組「送 ServerMsg 的 sink + 收 ClientMsg 的 stream」,WS 與 SSE+POST 各一個實作;agent 對話迴圈本身不變。
- client 端共用傳輸層(CLI 與 daemon 共用):先試 WS,連不到或 upgrade 失敗則退 SSE+POST。daemon 既有 backoff 重連迴圈納入傳輸選擇。
- 沿用既有 `Resume{conversation_id, after_seq}` 與事件 seq 對應 SSE 的 `Last-Event-ID`,做斷線無缺口續傳;SSE 串流加 heartbeat keepalive 與半開偵測。
- 認證:token 在 SSE 與 POST 兩條 HTTP 請求都以 `Authorization` header 帶上並驗證(沿用既有 AuthStore);`POST /send` 綁 session+token,防止他人灌訊息。
- 設定:新增 client 端設定可「強制走 SSE」或「關閉 SSE 備援」;client 從同一 host 推導 ws:// 與 http(s):// 兩種端點。

## Non-Goals

(本變更會建立 design.md,Non-Goals 寫在 design 的 Goals/Non-Goals 一節。)

## Capabilities

### New Capabilities

- `sse-transport-fallback`: SSE(下行)+ HTTP POST(上行)的備援傳輸、server 端 axum 多路(WS/SSE/POST)、兩條 HTTP 請求的 session 關聯、HTTP 通道認證、SSE keepalive 與 Last-Event-ID 續傳、以及 server 連線服務的傳輸抽象。

### Modified Capabilities

- `daemon-resilience`: 重連迴圈在選擇連線時,先試 WS、失敗再退 SSE+POST(原本只有 WS)。
- `device-enrollment`: 連線設定除了 WS URL,還要能推導/設定 SSE(http(s))端點,並新增「強制 SSE / 關閉備援」設定。

## Impact

- Affected specs: sse-transport-fallback(新)、daemon-resilience(改)、device-enrollment(改)
- Affected code:
  - New:
    - crates/fleety-server/src/http.rs(axum 前端 router:WS upgrade + GET /sse + POST /send + session 關聯)
    - crates/fleety-tools/src/transport.rs(client 端傳輸抽象:WS 與 SSE+POST 兩實作,CLI 與 daemon 共用)
  - Modified:
    - crates/fleety-server/src/conn.rs(serve 改吃泛型傳輸;accept 路徑移到 axum 後面)
    - crates/fleety-server/src/main.rs(listener 接 axum)
    - crates/fleety-server/Cargo.toml(新增 axum)
    - crates/fleety-cli/src/main.rs(改用共用傳輸,含 WS→SSE fallback)
    - crates/fleety-daemon/src/main.rs(連線/重連改用共用傳輸)
    - crates/fleety-daemon/src/backoff.rs(重連納入傳輸選擇)
    - crates/fleety-tools/Cargo.toml(新增 SSE client 所需依賴)
    - docs/env.md(SSE 端點與新設定變數)
    - README.md(傳輸/連線說明)
