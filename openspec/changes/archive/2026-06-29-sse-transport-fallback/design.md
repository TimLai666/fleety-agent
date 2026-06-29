## Context

裝置(daemon/CLI)與 server 目前只有單一傳輸:server 在裸 TCP listener 上對每條連線直接做 WebSocket 升級(fleety-server 連線處理模組的 accept 路徑),client 用 WS 連入。沒有 HTTP 框架,唯一網路依賴是 tokio-tungstenite。對話迴圈在一對 WS sink/stream 上以「讀 ClientMsg / 送 ServerMsg」運作,訊息是既有的 ClientMsg/ServerMsg JSON。

問題:當 WS 連不到(WS upgrade 被 proxy/防火牆擋,或環境只放行一般 HTTP),裝置無退路。SSE 是單向(server→client),不能單獨取代雙向的 WS,故備援必須是 SSE(下行)+ HTTP POST(上行)。

約束:工作區規則(forbid(unsafe_code)、never-crash errors-as-messages、agent-core 不依賴任何 fleety crate、env-var 測試可單執行緒跑);不改 ClientMsg/ServerMsg 的 JSON 形狀;WS 維持預設,SSE 純為備援。

## Goals / Non-Goals

**Goals:**

- WS 連不到時,client 自動退回 SSE+POST 並維持與 WS 等價的雙向對話能力。
- server 同一個 port 同時服務 WS、SSE、POST。
- 兩條 HTTP 請求(SSE 下行、POST 上行)以 session id 正確關聯到同一個邏輯連線。
- HTTP 通道沿用既有認證(token),不削弱 full_access 以外策略下的存取控制。
- 斷線後可無缺口續傳(重用既有 Resume + 事件 seq)。

**Non-Goals:**

- 不改 ClientMsg/ServerMsg 的 wire JSON 形狀。
- 不引入 NAT 穿透或反向連線。
- 不做 long-polling(只做 SSE+POST)。
- 不把 SSE 設為預設;WS 仍是首選,SSE 只在 WS 不可用時啟用。
- 不改 server 對話/工具迴圈本身的邏輯(只換它底下的傳輸)。

## Decisions

### axum 作為 server 前端 router 並多路 WS/SSE/POST

在 listener 前面放 axum router,路由三種請求:WS upgrade(`GET /` 帶 Upgrade)、`GET /sse`(下行串流)、`POST /send`(上行)。理由:axum 原生支援三者且維持單一 port;手刻 HTTP/SSE 在裸 TCP 上脆弱難維護。代價:新增 axum 依賴、accept 路徑重寫。axum WS 型別與 tungstenite 不同,故兩者都透過同一個傳輸抽象接到對話迴圈,不讓型別差異滲進 serve。

### 傳輸抽象讓 serve 與具體傳輸解耦

把 server 連線服務迴圈改成吃一組「送 ServerMsg 的 sink + 收 ClientMsg 的 stream」(以既有錯誤型別回報),而非 WS 專屬型別。WS 與 SSE+POST 各提供一個實作。對話迴圈(讀 Hello、跑回合、emit ServerMsg)邏輯不動。理由:同一套 agent 行為服務兩種傳輸,避免分叉。

### SSE 下行 + POST 上行,以 session id 關聯兩條請求

client 先以一次握手(POST /send 帶 Hello,或 GET /sse 的查詢參數)取得/宣告 session id;`GET /sse?session=…` 回傳該 session 的 ServerMsg 串流,`POST /send`(body 為單一 ClientMsg、帶 session id)灌入該 session 的上行通道。server 端為每個 SSE session 維護一個 inbound channel 與 outbound 串流,生命週期與一條 WS 連線等價。理由:SSE 單向,上行必須另走 POST;session id 是把兩條無狀態 HTTP 請求綁回一條邏輯連線的最小機制。

### HTTP 通道認證沿用 token,走 Authorization header

SSE 的 GET 與 POST 都帶 `Authorization: Bearer <token>`,server 沿用既有 AuthStore 驗證;未通過則拒絕建立/灌入 session。`POST /send` 必須同時符合 session id 與該 session 綁定的已驗證身分,避免他人對既有 session 灌訊息。理由:WS 把 token 放 Hello frame;HTTP 無持久連線,改放 header 是等價且標準的做法。

### 斷線續傳重用事件 seq 與 Resume,對應 Last-Event-ID

SSE 每筆事件帶 `id:`(對應該對話事件的 seq);client 重連時帶 `Last-Event-ID`(或 Resume{conversation_id, after_seq}),server 從該點之後補送,達成無缺口續傳。理由:已有 seq 與 Resume 機制,直接映射到 SSE 標準重連語意,不另造一套。

### SSE keepalive 與半開連線偵測

SSE 下行定期送 comment 心跳(keepalive),client 在逾時無心跳時視為斷線並觸發重連;server 偵測 outbound 寫入失敗即回收該 session。理由:SSE 經 proxy 容易半開,需主動心跳與逾時。

### client 端傳輸選擇:WS 優先,連不到再退 SSE

client 連線時先試 WS;若連線/upgrade 失敗(或設定強制 SSE),改走 SSE+POST。daemon 既有 backoff 重連迴圈納入此選擇(每輪先 WS 後 SSE),CLI 連線同理。理由:WS 較佳(全雙工、低負擔),只在不可用時退備援。

### client 共用傳輸層與雙端點 URL 推導

把 client 傳輸抽象放在 fleety-tools(CLI 與 daemon 共用),提供 WS 與 SSE+POST 兩實作及統一的「連線」介面回傳 sink/stream。client 從設定的 host 同時推導 ws(s):// 與 http(s):// 端點;新增設定可強制 SSE 或關閉備援。理由:CLI 與 daemon 連線邏輯一致,避免重複;URL 推導集中一處。

## Implementation Contract

**行為(Behavior):**

- 當 WS 可用:行為與現況完全相同(預設走 WS)。
- 當 WS 連不到而 SSE 可用:client 自動以 SSE+POST 連上 server,使用者可照常對話(送訊息、收串流回覆、approval、on-device RunTool),功能與 WS 等價。
- 當兩者都不可用:client 依既有 backoff 持續重試(WS 與 SSE 各試),錯誤以訊息回報、不崩潰。
- 斷線重連後不重複也不遺漏既有事件(以 seq 續傳)。

**介面 / 資料形狀:**

- server HTTP 端點:`GET /sse`(query 帶 session id;回 `text/event-stream`,事件 data 為 ServerMsg JSON,`id` 為事件 seq,定期送心跳 comment)、`POST /send`(body 為單一 ClientMsg JSON;header 帶 session id 與 `Authorization`)、WS upgrade 路由維持原行為。
- server 傳輸抽象:連線服務迴圈接受「ServerMsg 的非同步 sink」與「ClientMsg 的非同步 stream」;WS 與 SSE+POST 兩實作滿足之。
- client 傳輸抽象(fleety-tools):一個「connect」介面回傳上述 sink/stream 對,內部先 WS 後 SSE。
- 認證:HTTP 兩端點以 `Authorization: Bearer <token>` 驗證,沿用既有 AuthStore;沿用既有 ClientMsg/ServerMsg JSON,不新增/改欄位。
- 設定(client):強制 SSE 與關閉備援各一個 FLEETY_* 變數;agent host 同時推導 ws 與 http 端點。

**失敗模式:**

- token 缺/錯:HTTP 端點回 401/403,client 比照 WS 的 unauthenticated 處理(清除已存 token 後重試/重新配對)。
- POST /send 的 session 不存在或身分不符:拒絕(4xx),不灌入。
- SSE 心跳逾時或 outbound 寫入失敗:該 session 視為斷線,client 觸發重連,server 回收 session。
- WS 與 SSE 皆失敗:沿用 backoff 重試,錯誤以訊息回報,never-crash。

**驗收標準(Acceptance):**

- 單元測試:傳輸抽象的 WS 與 SSE+POST 實作各能完成「Hello→Welcome→一回合對話」往返(以記憶體/loopback 測 harness)。
- 單元測試:session 關聯(POST 灌入正確 session)、未授權被拒、Last-Event-ID 續傳從正確 seq 之後補送。
- 整合測試:對同一個 server,WS 與 SSE 兩種傳輸各跑通一次最小對話。
- 手動驗證:在會擋 WS upgrade 的環境(或以設定強制 SSE)確認 client 退回 SSE 後可正常對話。
- 既有 WS 路徑回歸測試全綠;clippy -D、單執行緒測試、agent-core host-free 維持。

**範圍邊界:**

- In scope:SSE+POST 傳輸、axum 多路、session 關聯、HTTP 認證、SSE 續傳/心跳、client WS→SSE 備援與共用傳輸層、相關設定與文件。
- Out of scope:改 wire 訊息形狀、NAT 穿透、long-polling、把 SSE 設為預設、改對話/工具迴圈邏輯。

## Risks / Trade-offs

- [新增 axum 依賴擴大 server 相依與編譯面] → 只在 server 端引入;client 傳輸用既有 reqwest;以最小 feature 引入 axum。
- [accept 路徑重寫可能回歸破壞既有 WS] → 先建立傳輸抽象並讓 WS 走新抽象,保留既有 WS 行為回歸測試;分階段(先抽象、再接 axum、最後加 SSE)。
- [SSE 經 proxy 半開或被緩衝] → 心跳 + client 逾時重連;事件帶 seq 確保續傳不漏。
- [POST 上行被冒灌] → session 綁已驗證身分 + token header 驗證。
- [雙傳輸增加維護面] → 共用同一傳輸抽象與同一套 ClientMsg/ServerMsg,行為單一來源。

## Migration Plan

- 分階段落地:(1) 抽出 server 傳輸抽象、WS 改走它(行為不變、可單獨上);(2) 引入 axum 並把 WS upgrade 接到 router(仍只有 WS);(3) 加 `GET /sse` + `POST /send` 與 session 管理;(4) client 共用傳輸層 + WS→SSE 備援 + 設定;(5) 文件。
- 相容:舊 client 仍走 WS,server 同 port 同時支援;無資料格式變更,無需資料遷移。
- 回滾:SSE 端點與 client 備援為附加;停用備援設定或移除 SSE 路由即回到純 WS。

## Open Questions

- session id 的產生與壽命上限(閒置多久回收)細節在 apply 時定;預設沿用 WS session 的既有語意。
- 強制 SSE / 關閉備援的設定變數命名(FLEETY_*)在 specs/tasks 階段定稿。
