## 1. Server 傳輸抽象(transport-agnostic loop)

- [x] 1.1 定義 server 端傳輸抽象(一組 ServerMsg sink + ClientMsg stream)並讓現有 WebSocket 改走此抽象,交付 "Server connection loop is transport-agnostic";對應設計「傳輸抽象讓 serve 與具體傳輸解耦」。先寫失敗測試:以記憶體 loopback 傳輸驅動連線服務迴圈跑 Hello→Welcome→一回合對話;驗證既有 WebSocket 連線整合測試全綠(行為回歸不變)。

## 2. axum 前端 router

- [ ] 2.1 引入 axum 並把 WebSocket upgrade 接到 axum router(本階段僅 WS 路由),交付 "Server multiplexes WebSocket, SSE, and POST on one port" 的同 port 多路基礎;對應設計「axum 作為 server 前端 router 並多路 WS/SSE/POST」。驗證:WebSocket 經 axum 仍跑通最小對話的整合測試;clippy -D 與既有測試維持綠。

## 3. SSE+POST 傳輸與 session 關聯

- [ ] 3.1 實作 `GET /sse`(下行 ServerMsg 串流,事件帶對話事件 seq 作為 id)與 `POST /send`(上行單一 ClientMsg),以 session id 關聯兩條 HTTP 請求,交付 "SSE plus POST provides a bidirectional transport via session correlation";對應設計「SSE 下行 + POST 上行,以 session id 關聯兩條請求」。先寫失敗測試:建立 session、開 SSE、POST Hello→收到 Welcome 與串流回覆;另測兩個 session 各自 POST 只路由到自己。
- [ ] 3.2 讓 SSE+POST 滿足 task 1.1 的 server 傳輸抽象並與 WebSocket 共用同一連線服務迴圈,交付 "Server connection loop is transport-agnostic" 的第二個傳輸實作。驗證:同一最小對話分別經 WebSocket 與 SSE+POST 各跑一次、結果等價的整合測試。

## 4. HTTP 通道認證

- [ ] 4.1 對 `GET /sse` 與 `POST /send` 以 `Authorization` header 驗證 token(沿用既有 AuthStore),並要求 `POST /send` 的 session id 必須綁定該已驗證身分,交付 "HTTP transport authentication";對應設計「HTTP 通道認證沿用 token,走 Authorization header」。先寫失敗測試:未授權的 SSE 被拒(unauthorized);POST 到非本身身分綁定的 session 被拒、訊息不灌入。

## 5. 續傳與 keepalive

- [ ] 5.1 SSE 事件帶 seq,並依 `Last-Event-ID` 或 `Resume{conversation_id, after_seq}` 從該 seq 之後續傳,交付 "Gap-free resumption over SSE";對應設計「斷線續傳重用事件 seq 與 Resume,對應 Last-Event-ID」。先寫失敗測試:client 已收到至 seq N、斷線重連後 server 只送 seq > N、不重不漏。
- [ ] 5.2 SSE 下行定期送 keepalive comment、server 在下行寫入失敗時回收該 session、client 在逾時無心跳時視為斷線觸發重連,交付 "SSE keepalive and half-open detection";對應設計「SSE keepalive 與半開連線偵測」。驗證:半開逾時觸發重連的單元測試;server 寫入失敗回收 session 的測試。

## 6. Client 傳輸層、fallback 與設定

- [ ] 6.1 在 fleety-tools 實作 client 共用傳輸層(WebSocket 與 SSE+POST 兩實作 + 統一 connect 回傳 sink/stream)並從單一 host 推導 ws(s):// 與 http(s):// 端點,對應設計「client 共用傳輸層與雙端點 URL 推導」,並交付 "Daemon connection configuration" 的 SSE 端點推導面。先寫失敗測試:從一個 host 正確推導兩種端點;connect 在 WebSocket 失敗時改選 SSE(以可注入的假傳輸驗證選擇路徑)。
- [ ] 6.2 [P] daemon 重連迴圈改用共用傳輸層,每輪先試 WebSocket、失敗(或被擋)再退 SSE+POST,交付 "The daemon reconnects after disconnect or sleep" 的傳輸選擇;對應設計「client 端傳輸選擇:WS 優先,連不到再退 SSE」。驗證:重連傳輸選擇單元測試(WebSocket 失敗→選 SSE)。
- [ ] 6.3 [P] CLI 連線改用共用傳輸層(同樣 WebSocket 優先、失敗退 SSE),交付 CLI 在 WS 被擋環境仍可連線對話。驗證:CLI 連線選擇單元測試;手動驗證在強制 SSE 下可對話。
- [ ] 6.4 新增「強制 SSE」與「關閉 SSE 備援」兩個 FLEETY_* 設定變數,完成 "Daemon connection configuration" 的設定面;對應設計「client 端傳輸選擇:WS 優先,連不到再退 SSE」。驗證:設定解析單元測試(強制 SSE 時跳過 WebSocket;關閉備援時 WebSocket 失敗即不退 SSE)。

## 7. 文件

- [ ] 7.1 [P] 更新 docs/env.md 與 README.md,記錄 `GET /sse`、`POST /send` 端點、WebSocket→SSE 備援行為與「強制 SSE / 關閉備援」設定變數。驗證:內容審查,確認上述端點、fallback 與設定皆有涵蓋。
