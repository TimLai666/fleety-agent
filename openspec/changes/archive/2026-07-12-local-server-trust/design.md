## Context

Server 認證在 `conn::authenticate`(Hello 的 token/pairing 檢查),由 `run_connection` 呼叫。兩個 `run_connection` 呼叫點都在 axum handler(`http::serve_ws` 的 WS 與 `sse` 的 POST 路徑),所以 peer 位址可用 axum `ConnectInfo<SocketAddr>` 取得——目前 serve 用 `app.into_make_service()`(不帶 ConnectInfo)。`FLEETY_REQUIRE_AUTH` 預設開;`authenticate` 在 `auth.required()` 為真且無有效 token/pairing 時拒絕。install-server.sh 目前只抓 server 與 sidecar。CLI resolver 優先序:override > env > current profile(sticky)> mDNS > localhost 預設(`ws://127.0.0.1:8787`);引導式 `fleety init` 掃 LAN 選單配對。

威脅模型現況:wire 是 ws:// 明文,同機行程能讀 server 的 home(token、config、providers)。因此「同機 loopback 連線」在信任層級上等同「能讀那些檔的行程」——要求它再出示 token 不增安全性,只增摩擦。

## Goals / Non-Goals

**Goals:**
- 裝 server 的主機自動有 CLI。
- 本機 CLI 連本機 server 免配對、免手動 URL、自動、當一級可切換 profile。
- 掃到本機 server 預設連本機,可切換遠端;設定落點沿現況。
- 遠端/LAN 連線的認證行為完全不變。

**Non-Goals:**
- 不引入 TLS 或挑戰式驗證(既有後續)。
- 不改遠端連線的認證需求。
- 不改設定落點邏輯(current profile 指哪落哪,已符合需求)。
- 不對 SSE 以外新增傳輸;loopback 信任對 WS 與 SSE 兩條 axum 路徑一致適用。

## Decisions

### 決策一:loopback 信任在 authenticate,peer 位址由 ConnectInfo 帶入

serve 改用 `into_make_service_with_connect_info::<SocketAddr>()`;`ws_handler` 與 `sse`/`send` handler 取 `ConnectInfo<SocketAddr>`,算出 `peer_is_loopback`(`ip.is_loopback()`),一路帶進 `run_connection` → `authenticate`。`authenticate` 新規則:`auth.required() && peer_is_loopback && trust_loopback_enabled()` → 免 token 放行(回 `Ok(None)`)。其餘不變。`trust_loopback_enabled()` 讀 `FLEETY_TRUST_LOOPBACK`(預設真,`0` 為關)。

- 以連線 socket 的 peer 位址判定,不看任何可偽造的請求欄位(Host/X-Forwarded-For 一律不採)。反向代理若把遠端連線以 loopback 轉發會誤判——文件明載:置於代理後要 `FLEETY_TRUST_LOOPBACK=0` 或讓代理保留真實 peer。
- 否決「CLI 直接讀 server 的 token 檔自我認證」:跨行程讀 secret 檔、且要 CLI 知道 server home 路徑,較脆;loopback 信任在 server 端一處判定更乾淨。

### 決策二:Welcome 帶回 loopback 信任旗標

`Welcome` 加 `loopback_trusted: bool`(additive,default false):server 在以 loopback 信任放行時設真。CLI 據此在 `fleety init` 得知本機 server 免配對(選它時不提示配對碼),也讓未來的 UX 能標示「已用同機信任連入」。

### 決策三:install-server.sh 一併裝 CLI

在裝完 server 與 sidecar 後,以相同的 target/釋出資產邏輯抓 `fleety` 裸資產或壓縮檔並安裝到同一 `dir`。best-effort:失敗印一行提示(可手動跑 install.sh),不影響 server 安裝結果。裝完後尾段提示改為「本機已有 CLI:直接 `fleety init` 掃描選本機」。

### 決策四:CLI 把本機 server 當一級預設可切換 profile

引導式 `fleety init`(無 URL、TTY)流程調整:先探測本機 server(嘗試連 `ws://127.0.0.1:<port>`,port 取 `FLEETY_ADDR` 的 port 或預設 8787),若 `Welcome` 回來就把它當一個 `DiscoveredServer`(名稱 `local`、標記),**列在選單頂端且為預設選項**;再併入 LAN 掃描結果。選 `local` 時:存 `local` profile(URL=本機)、設 current、**跳過配對碼提示**(loopback 信任)。選遠端沿既有(存 profile、提示配對)。runtime:resolver 的 localhost fallback 既已存在,加上 loopback 信任,沒有 current profile 的主機 CLI 直接連本機免配對。切換遠端用 `fleety server use <name>`(sticky);切回本機 `fleety server use local`。

- 探測用短逾時(~1s)避免無本機 server 時拖慢 init。
- 否決「每個指令都探測本機」:只在 init 設定期探測;runtime 靠既有 resolver 優先序 + localhost fallback。

## Implementation Contract

**行為(操作者視角):**
- 一行 install-server.sh 後,主機同時有 `fleety-server` 與 `fleety`。
- 該主機 `fleety init`:「Scanning…」後選單頂端是 `1. local  ws://127.0.0.1:8787  (local, no pairing)`,Enter 直接選它 → 「Using 'local' … as the current server.」無配對碼提示 → 立即可 `fleety tui`/`ask`。
- 本機 server `FLEETY_REQUIRE_AUTH=1` 仍成立:遠端裝置照舊要配對;只有同機 loopback 免。
- `FLEETY_TRUST_LOOPBACK=0`:連本機也要 token(多租戶主機用)。
- 設定:`fleety config …`、`provider edit`、`auth login` 全落在目前 current profile 指向的 server(本機或切換後的遠端)——邏輯不變。

**介面與資料形狀:**
- `run_connection` / `authenticate` 增 `peer_is_loopback: bool` 參數(測試呼叫傳明確值)。
- `Welcome` 增 `loopback_trusted: bool`(`#[serde(default)]`,additive)。
- `trust_loopback_enabled()`(讀 `FLEETY_TRUST_LOOPBACK`,預設真)、`peer_is_loopback` 判定純函式(`SocketAddr` → bool)。
- CLI:`local` profile 名為約定值;本機探測 helper(短逾時連 localhost、回是否有 server)。
- install-server.sh:抓 `fleety` 資產的區塊(沿 server 的 target 對照)。

**失敗模式:**
- 無本機 server 時 init 探測逾時 → 略過 local、照常 LAN 掃描/usage。
- CLI 安裝失敗(install-server.sh)→ 提示手動裝,server 安裝仍成功。
- 代理誤把遠端當 loopback → 文件指引關閉信任(非程式可自動偵測,列風險)。

**驗收準則:**
- cargo test:`peer_is_loopback` 純函式(v4/v6 loopback vs 非 loopback);`authenticate` 在 loopback+require_auth+trust 放行、trust 關閉時仍拒、遠端仍拒的分支;`trust_loopback_enabled` env 解析;`Welcome` additive round-trip(protocol)。
- 既有 auth 測試不回歸(遠端行為不變)。
- install-server.sh `sh -n` 通過;CLI 安裝區塊人工比對。
- 全 workspace test/clippy/fmt 乾淨。
- 端到端(發版後人工):Mac mini install-server 後,同機 `fleety init` 選 local 免配對連上。

**範圍邊界:**
- 範圍內:crates/fleety-server/src/{http.rs,conn.rs}、crates/fleety-protocol/src/lib.rs、crates/fleety-cli/src/main.rs、scripts/install-server.sh、docs/env.md。
- 範圍外:TLS、pairing code 鑄造(Change 2)、TUI 重連(Change 2)、resolver 優先序重寫、SSE 以外傳輸。

## Risks / Trade-offs

- [反向代理把遠端連線以 loopback 轉發 → 誤信任] → 文件明載,置代理後設 `FLEETY_TRUST_LOOPBACK=0`(或代理保留真實 peer)。預設信任是為「絕大多數單機/家用」順手;企業多租戶關掉。
- [同機惡意行程免配對連入] → 該行程本就能讀 token/config 檔,信任層級相同,非新增暴露面。
- [init 本機探測拖慢無 server 的主機] → 短逾時(~1s)+ 僅設定期。

## Migration Plan

單版出貨。存量主機:升級後重跑 install-server.sh 取得 CLI(或手動 install.sh);loopback 信任對既有遠端部署零影響。回滾 revert。

## Open Questions

- 無阻斷項。
