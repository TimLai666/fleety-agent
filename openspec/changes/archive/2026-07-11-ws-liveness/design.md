## Context

WebSocket 是主要傳輸（axum 在單一 listen port 上服務 WS + SSE + POST）。server 端每條連線由 transport-agnostic 的 run_connection 驅動，WS 與 SSE 只差在 inbound（ClientInbound）與 outbound（FrameWriter）adapter；連線結束時已有完整清理路徑（Hub 與 device_tools 移除、writer task 結束）。SSE 路徑已有 keepalive 與 45 秒 half-open 偵測；WS 路徑目前完全沒有：server 不發 ping、共用 client transport（fleety CLI 與 fleetyd 同用）的 WS 接收端沒有 read deadline。另一個關鍵現況：fleetyd 收到 RunTool 是在 serve 迴圈內 inline 執行工具，執行期間不讀 socket，因此不會回 pong；而 server 端 device_exec 對單次呼叫本來就有 30 秒逾時。既有 spec 約束：server 連線迴圈必須維持 transport-agnostic（sse-transport-fallback 規範），所以 liveness 必須落在 adapter 層之下，不能進 run_connection。

## Goals / Non-Goals

**Goals:**

- 靜默斷線（睡眠、NAT idle、Wi-Fi 切換）後，server 在存活期限內把裝置從 Hub 移除，device_exec 立即 fail-fast 而不是燒滿呼叫逾時。
- daemon / CLI 端的半開 WS 在存活期限內被偵測為斷線，走既有重連或 link-closed 路徑，不再永久掛起。
- 舊版 fleetyd 不升級即可被新 server 正確偵測（tungstenite 幀層自動回 pong）。
- 新版 client 對舊版 server（不發 ping）行為與現況完全相同，無誤判。

**Non-Goals:**

- 不改 SSE 傳輸（keepalive 與 45 秒偵測已存在且足夠）。
- 不改 fleety-protocol 的訊息集或 protocol version。
- 不改 fleetyd 的工具執行模型（inline 執行維持現狀；併發化是獨立的未來變更）。
- 不做 Hub 層級的週期掃描器（per-connection 偵測已讓既有清理路徑收斂，第二套真相來源只會漂移）。
- 不動 presence / co-location 語義（liveness 只是它未來可用的底層訊號）。

## Decisions

### 用 WS 幀層 Ping/Pong，不改 fleety-protocol

liveness 用 WebSocket 協定內建的 Ping/Pong 控制幀，而不是新增 ClientMsg/ServerMsg 訊息。理由：(1) 舊版 client 的 tungstenite 在讀取時自動回 pong，整個 fleet 不需升級就能被偵測；(2) 訊息集與 protocol version 不變，沒有相容性面積；(3) 控制幀天然位於 run_connection 的抽象之下，符合「連線迴圈 transport-agnostic」的既有規範。捨棄的替代方案：protocol 層 Ping/Pong 訊息（要雙端都升級才生效，且污染訊息集）；TCP keepalive（OS 預設以小時計、不可攜、非端到端）。

### server 端以 socket-owner task 承載 liveness

重構 http.rs 的 serve_ws：一個 task 獨占整個 socket，以 select! 同時處理「outbound 通道要送的文字幀、socket 入站幀、ping 週期、存活期限檢查」；WS 的 ClientInbound / FrameWriter adapter 改為 mpsc 通道形式（與 SSE adapter 同構）。ping 逾期或 socket 結束時 task 收束，inbound 端回 None，run_connection 照既有路徑 unwind 並清理 Hub。理由：sink/stream 被 run_connection 分持，獨立 ping task 拿不到寫入端；Hub 的 out 通道雖是 WsMessage 但 writer task 只轉發 Text 幀，塞 Ping 進去會被丟棄。socket-owner task 把所有 liveness 邏輯集中在一處，且讓 WS 與 SSE 兩條路徑的 adapter 形狀一致。

### 任何入站幀都重設存活期限

server 端的期限以「最後一次收到任何幀」計，不只認 pong：文字幀、pong、close 都算活著。理由：正在串流結果的連線不該因 pong 排程被誤殺；期限語義是「這條線還有東西流動嗎」，不是「對方有沒有準時回 pong」。

### 客戶端 read deadline 採 ping-adaptive 啟用

共用 transport 的 WS 接收端：同一條連線收到第一個 server Ping 之後才啟用 read deadline；啟用後每次等待幀都套逾時，任何幀（含 Ping）重設；逾期回報連線結束（recv 得到 None），fleetyd 走既有指數退避重連、CLI 走既有 link-closed 處理。理由：fleet 收斂是 forward-only，「新 daemon + 舊 server」會長期存在，無條件啟用 deadline 會讓不發 ping 的舊 server 底下的閒置裝置每 60 秒誤判斷線一次，形成重連風暴。捨棄的替代方案：在 Welcome 加能力欄位宣告（動 protocol，且 ping 本身就是最誠實的能力證明）。

### 時序預設 ping 20 秒、期限 60 秒，登錄 config registry

server ping 週期預設 20 秒（FLEETY_WS_PING_SECS，Server scope）；存活期限雙端共用預設 60 秒（FLEETY_WS_TIMEOUT_SECS，Shared scope）。60 秒 = 連續三次 ping 無回應，遠低於常見 NAT idle 逾時，與 SSE 的 45 秒姿態一致；期限必須大於 ping 週期兩倍以上。解析語義沿用 FLEETY_SSE_TIMEOUT_SECS 慣例：非正整數視為未設定、落回預設，不提供停用開關（要放寬就調大）。兩個鍵都進 config.rs 的 curated registry 附驗證器，並補 docs/env.md。

### 接受忙碌 daemon 的已知互動，不改工具執行模型

fleetyd inline 執行工具期間不讀 socket、不回 pong，超過 60 秒的工具會讓 server 踢掉這條連線。這是刻意接受的取捨：該次呼叫早在 30 秒的 device_exec 逾時就已對 agent 失敗；工具在裝置本地照常跑完（副作用保留），daemon 送結果失敗後走既有重連，數秒內回到 Hub。相比之下，把工具執行 spawn 出去會改變裝置端併發語義（多工具同時跑），屬於獨立變更。本決策要在程式註解與 env 文件中明示這個互動。

## Implementation Contract

**可觀察行為：**

- 閒置且健康的 WS 連線（會回 pong）永遠不被 server 踢除，跨越任意多個 ping 週期仍可路由。
- 裝置端靜默消失（不讀不寫、無 FIN/RST）後，最遲「存活期限 + 一個 ping 週期」內：server 端該連線結束、Hub 與 device_tools 條目移除、對該裝置的 device_exec 立即回「not connected」類錯誤而不是等 30 秒。
- server 靜默消失後，已啟用 deadline 的 fleetyd 最遲在存活期限內偵測到並進入既有退避重連；CLI 端 recv 得到連線結束。
- 舊 fleetyd + 新 server：不升級即受偵測保護（自動 pong）。新 client + 舊 server：全程無 deadline，行為與本變更前完全一致。
- SSE 連線的行為完全不變。

**介面／資料形狀：**

- 新 env 鍵：FLEETY_WS_PING_SECS（預設 20）、FLEETY_WS_TIMEOUT_SECS（預設 60），非正整數落回預設；registered config entry 各附描述與驗證器；docs/env.md 各補一列。
- fleety-protocol 零改動（訊息集、版本、序列化都不變）。
- fleety-tools transport 的公開簽名不變：Receiver 的 recv 在 deadline 逾期時回 None（與既有「連線關閉」同一形狀），呼叫端無需新分支。

**失敗模式：**

- 期限逾期是正常斷線，不是錯誤：server 端記一筆含裝置識別與原因（liveness timeout）的 log 後走既有清理；client 端 recv 回 None。整條路徑維持 never-crash、errors-as-messages。
- ping 發送失敗視同連線已死，立即收束（不等期限）。

**驗收標準（實作與審查都以此確認）：**

- server 端測試（http.rs 測試模組，用短時序參數）：(a) 健康閒置的 tokio-tungstenite client 存活超過兩倍期限仍在 Hub、可被路由；(b) 完成 Hello 後停止輪詢 socket 的 client（模擬半開／忙碌）在期限加一個 ping 週期內被移出 Hub，且對其 device_exec 立即失敗。
- client 端測試（transport.rs 測試，配可控的假 server）：(c) 先 ping 後靜默的 server，recv 在期限內回 None；(d) 從不 ping 的 server，recv 掛等超過期限窗口仍不誤判（deadline 未啟用）。
- 設定測試：(e) 兩個 env 鍵在 registry 中存在、預設值正確、非法值被驗證器拒絕。
- 手動 E2E（選做，release 前煙霧測試）：真實 server + fleetyd，以防火牆規則靜默丟包，觀察雙端在期限內偵測並重連；對照組確認修前是 30 秒逾時／永久掛起。

**範圍邊界：**

- In scope：crates/fleety-server/src/http.rs 的 WS adapter 重構與 liveness、crates/fleety-tools/src/transport.rs 的 WS 接收端 deadline、crates/fleety-tools/src/config.rs 的兩個 registry 條目、docs/env.md、上述測試。
- Out of scope：SSE 路徑、fleety-protocol、fleetyd 工具執行模型、conn.rs 的 run_connection 與清理邏輯（沿用不改）、CLI 的斷線 UX、Hub 掃描器、presence。

## Risks / Trade-offs

- [中介設備吞 WS 控制幀：server ping 到不了 client] → client 端不啟用 deadline（看不到 ping 就不武裝），無誤判；但 server 收不到 pong，閒置連線會每個期限週期被踢一次形成重連循環。RFC 合規的 proxy 必須轉發控制幀，此情境罕見；緩解：調大 FLEETY_WS_TIMEOUT_SECS，或以 FLEETY_FORCE_SSE=1 改走已有 keepalive 的 SSE 路徑。docs/env.md 註明。
- [長工具讓忙碌 daemon 被踢]（見「接受忙碌 daemon 的已知互動」決策）→ 影響有界：呼叫早已逾時失敗、副作用保留、工具結束後數秒內重連回 Hub；文件明示。
- [split 半流的自動 pong 時序：tokio-tungstenite 的 auto-pong 由讀取端輪詢觸發，若讀取端停擺 pong 就不流] → 這正是我們「要」偵測的狀態，語義自洽；健康路徑由驗收測試 (a) 直接覆蓋，若 auto-pong 在 split 下不如預期，該測試會在實作期立即暴露。
- [時序參數誤設（期限 ≤ ping 週期）造成健康連線被踢] → registry 驗證器拒絕期限小於兩倍 ping 週期的組合不可行（兩鍵獨立設定），改為各自驗證正整數＋文件寫明建議關係；預設值本身安全。
- [部署順序] → 無遷移需求：先升 server 或先升 client 都安全（相容矩陣見 ping-adaptive 決策），無持久化狀態、無協定變更，回滾即恢復原行為。
