## Why

CLI 應該能在 server 主機上直接用,但目前兩個障礙:安裝 server 的腳本只裝 server,主機上根本沒有 `fleety`;而且就算手動裝了,server 預設要求認證,本機 CLI 連自己主機的 server 還要配對——同機的荒謬。使用者要的是:裝 server 時一併裝 CLI,CLI 對本機 server 免配對、自動連上、當作其中一台 server 處理(設定改動照樣落在目前指向的 server,無需改變),而且掃到本機 server 時預設連本機、但可切換到遠端。

## What Changes

- **install-server.sh 一併安裝 `fleety` CLI**(與 server 同一 release/target 邏輯,best-effort;失敗只提示,不影響 server 安裝)。
- **Server 信任 loopback 連線**:來自同機(127.0.0.1 / ::1)的連線即使 `FLEETY_REQUIRE_AUTH=1` 也免 token/配對——同機行程本就能讀 server 的 token 與 config 檔,要求 token 不增任何安全性。以 `FLEETY_TRUST_LOOPBACK` 控制(預設開,可設 `0` 關閉)。連線的 peer 位址由 axum `ConnectInfo` 一路帶進 `run_connection` 與 `authenticate`;`Welcome` 帶回是否以 loopback 信任連入(供 CLI 決定是否需要配對引導)。
- **CLI 把本機 server 當一級可切換 profile**:`fleety init`(無 URL、TTY)先探測本機 server(loopback),有回應就在選單頂端列出(標 `(local)`、預設選項),選它**不需配對碼**(loopback 信任),存成 `local` profile 並設為 current;LAN 掃描到的其他 server 照舊列在下方。runtime 連線沿用既有 resolver 的 localhost fallback(現在因 loopback 信任而免配對即可連)。切換到遠端用既有 `fleety server use <name>` / 選單重跑,設定落點沿現況(落在目前 current profile 指向的 server)。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `authentication-default-on`: 新增 loopback 信任——同機連線在認證開啟時仍免 token,由 `FLEETY_TRUST_LOOPBACK` 控制;peer 位址判定與威脅模型說明。
- `connection-profiles`: 本機 server 成為 CLI 的一級、預設、可切換 profile(`local`);免配對取得(靠 loopback 信任)。
- `service-discovery`: 引導式 init 在 LAN 掃描前先探測並在選單頂端列出本機 server(預設選項、免配對)。
- `self-update`: install-server.sh 一併安裝 CLI(擴充「Sidecar and install paths」的安裝行為)。

## Impact

- Affected specs: `authentication-default-on`、`connection-profiles`、`service-discovery`、`self-update`
- Affected code:
  - Modified:
    - crates/fleety-server/src/http.rs
    - crates/fleety-server/src/conn.rs
    - crates/fleety-protocol/src/lib.rs
    - crates/fleety-cli/src/main.rs
    - scripts/install-server.sh
    - docs/env.md
  - New: （無）
  - Removed: （無）
- 相容性:`Welcome` 的 loopback 標記為 additive 可選欄位;`FLEETY_TRUST_LOOPBACK` 未設即預設信任(向後相容,遠端連線行為完全不變——只有同機 loopback 受影響)。既有 `fleety init <url>`、`pair`、resolver 優先序不變。
- 安全:loopback 信任只放行真正的同機 peer(以連線 socket 的 peer 位址判定,非 Host 標頭、非可偽造欄位);LAN 連線一律照舊要求認證;可用 `FLEETY_TRUST_LOOPBACK=0` 對多租戶主機關閉。
