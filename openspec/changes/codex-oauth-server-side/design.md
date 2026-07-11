## Context

現況三段式：`fleety auth login`（crates/fleety-cli/src/auth.rs）在 CLI 端跑 PKCE（固定 loopback port 1455、驗 state、換 token），把 `Tokens`（access/refresh/expiry/account_id）存進**CLI 那台**的 `~/.fleety/codex-oauth.json`（oauth::default_token_path，受限權限）。server 端消費在 crates/fleety-server/src/providers.rs：provider 為 oauth:codex 時建 `OAuthCodexAuth::new(default_token_path(), &config)`——讀的是 **server 那台**的同名路徑，並在使用時自動 refresh。CLI 與 server 不同機時兩個路徑是兩台機器，登入結果 server 看不到。

既定架構事實：「所有 node 憑證一律存 Agent（server）端 secret store」是拍板決策；config 面已遠端化（fleety-protocol 的 `ConfigExec` 加上結構化 `ConfigSnapshot`/`ConfigApply`，Welcome 以 `config_protocol` 欄位（`CONFIG_PROTOCOL_VERSION = 1`，舊 server 缺省視為 0）做能力協商；auth-default-on 規定遠端 mutating 寫入必須在認證開啟的連線上）。`OAuthCodexAuth` 的 token 路徑是建構時注入的，server 消費端與 refresh 邏輯完全不用動。

限制：Codex 的 redirect URI 註冊死 `http://localhost:1455/auth/callback`，授權頁必須開在使用者面前——PKCE 流程不可能搬到 server 端。token 經 WS 傳輸的信任邊界與現有 device token、config 寫入相同（同一條已配對認證的連線）。

## Goals / Non-Goals

**Goals:**

- `fleety auth login/status/logout` 作用於**連線中的 server**：login 在 CLI 端完成 PKCE 後把 tokens 交付 server 落地，status/logout 查詢／清除 server 端憑證，CLI 不再落地 token 檔。
- 協定 additive：舊 CLI×新 server、新 CLI×舊 server 都不壞；新 CLI 對舊 server 在 auth 子指令給明確的版本不足錯誤。
- 憑證寫入／清除留 audit；auth 關閉的 server 拒絕遠端憑證寫入（沿 auth-default-on 原則）。
- CLI 端舊殘檔有遷移出路（login 成功後清掉並提示）。

**Non-Goals:**

- 不動 PKCE 流程、authorize URL、port 1455、token exchange。
- 不動 server 端消費鏈與 refresh（OAuthCodexAuth、plan_bearer 照舊）。
- 不做 keychain／通用 secret manager（受限權限檔維持，keychain 是既列後續項）。
- 不做多 server 憑證同步、不加 `--local` 本機儲存逃生口。
- 不把憑證塞進 config key-value 面（見決策一）。

## Decisions

### 決策一：憑證專用 frame 組

新增三對 frame（ClientMsg → ServerMsg）：`CredentialPut { kind, payload }` → `CredentialResult`、`CredentialStatus { kind }` → `CredentialStatusResult`、`CredentialDelete { kind }` → `CredentialResult`。`kind` 是字串鑑別子（本次僅 `"codex-oauth"`，未知 kind 回明確錯誤），`payload` 是該 kind 自定的 JSON（codex-oauth 即 Tokens 的欄位）。`CredentialStatusResult` 只含存在與否、過期時間戳、account 標示，**永不含 token 值**。

- 否決「重用 ConfigApply 塞 config key」：config 面有 snapshot/list/edit 介面，secret 進去就會出現在面板與 snapshot 流（即使遮蔽也擴大暴露面）；且 config 的 optimistic-lock 語義對「整組憑證原子交付」是錯的形狀。
- 否決「泛化成任意 secret store API」：目前只有一個 kind，先以 kind 鑑別預留擴充即可，避免為未來想像過度設計。

### 決策二：PKCE 留在 CLI，tokens 經連線交付

login 流程前半不變（bind 1455 → 開瀏覽器 → 換 token）；拿到 Tokens 後不寫本機檔，改為建立（或沿用）到 current profile server 的已認證連線，送 `CredentialPut`。交付成功的訊息指名 server（profile 名＋URL）；交付失敗（連不上、未配對、server 拒絕）則整個 login 失敗並給 remediation（先 `fleety pair`、或升級 server），**不退回本機儲存**——半套成功比明確失敗更糟。

### 決策三：server 落地沿用既有檔案與消費端

server 端 handler 收 `CredentialPut(kind=codex-oauth)`：反序列化驗 payload 形狀（缺欄位即拒），呼叫既有 `oauth::save_tokens(default_token_path(), …)` 寫入（Unix 0600 照舊）。`CredentialDelete` 走 `oauth::clear_tokens`。消費端 providers.rs 與自動 refresh 一行不改——refresh 後的新 token 也由 server 自己寫回同一檔案（現行行為）。

### 決策四：config_protocol 版本 bump 到 2

credential frames 視為 config 遠端面的延伸能力：`CONFIG_PROTOCOL_VERSION` 由 1 bump 為 2，Welcome 的既有 `config_protocol` 欄位自然帶到。CLI 在 auth 子指令先看握手回報的版本，`< 2` 即報「server 版本過舊，先更新 server（fleety update）再登入」，不送 frame、不靜默降級。否決新增獨立版本欄位：Welcome 已有現成欄位與比較邏輯，兩個版本數字徒增心智負擔。

### 決策五：CLI 殘檔遷移

login 成功交付後：若 CLI 本機存在舊 `codex-oauth.json`，刪除並印一行提示（「本機殘留的舊憑證檔已移除；憑證現在存放於 server」）。`auth status` 若發現本機殘檔，提示它已不再被任何流程讀取、建議重新 `fleety auth login`。不做自動上傳殘檔（舊檔可能過期或屬於別的帳號，重跑 login 才是正路）。

### 決策六：audit 與 auth 前置

server 端把憑證寫入／清除記進既有 audit 機制（事件含 kind 與來源裝置，不含 token 值）。依 auth-default-on 的「遠端寫入⇒認證必開」：`FLEETY_REQUIRE_AUTH=0` 的 server 拒絕 `CredentialPut`/`CredentialDelete`（回 WireError 指名開 auth 的做法），`CredentialStatus` 同樣拒絕（狀態也是敏感面）。

## Implementation Contract

**行為（操作者視角）：**

- `fleety auth login`：瀏覽器授權 → 成功後印「Signed in. Credentials delivered to server '<profile>' (<url>).」；CLI 機器上不產生 `~/.fleety/codex-oauth.json`。
- server 未配對／不可達：login 在交付階段失敗，錯誤含 remediation（先 `fleety pair <code>` 或檢查連線）；PKCE 已完成但 token 不落地任何地方。
- 舊 server（config_protocol < 2）：auth 子指令直接報版本不足與升級指引，不進 OAuth 流程（login 在開瀏覽器**之前**就檢查，避免白跑授權）。
- auth 關閉的 server：拒絕並回「enable FLEETY_REQUIRE_AUTH and pair this device to manage credentials remotely」語意的錯誤。
- `fleety auth status`：印連線中 server 的登入狀態（signed in / not signed in、過期時間）；本機殘檔存在時附一行提示。
- `fleety auth logout`：清除 server 端 token 檔並記 audit；印「Signed out on server '<profile>'」。
- server 端模型呼叫行為完全不變（同機情境下與現狀位元級相同：同一路徑、同一格式）。

**介面與資料形狀：**

- fleety-protocol：`ClientMsg::CredentialPut { kind: String, payload: serde_json::Value }`、`ClientMsg::CredentialStatus { kind: String }`、`ClientMsg::CredentialDelete { kind: String }`；`ServerMsg::CredentialResult { ok: bool, error: Option<WireError> }`、`ServerMsg::CredentialStatusResult { present: bool, expires_at_secs: Option<u64>, detail: Option<String> }`（實際欄位命名 apply 時對齊鄰近 frame 慣例）。`CONFIG_PROTOCOL_VERSION` = 2。
- codex-oauth 的 payload 形狀 = 既有 `oauth::Tokens` 的 serde 形狀（單一真相：直接 serialize Tokens，server 端 deserialize 回 Tokens，不另定義 wire 結構）。
- `oauth::default_token_path()`、`save_tokens`、`clear_tokens`、`load_tokens` 簽名不變；CLI 端 auth.rs 停止呼叫 save_tokens（僅殘檔清理用 clear_tokens）。

**失敗模式：**

- 未知 kind → CredentialResult error「unsupported credential kind」。
- payload 缺欄位／型別錯 → error 指名缺什麼，不寫入任何東西。
- 寫檔失敗（權限/磁碟）→ error 帶 OS 錯誤，audit 記失敗事件。
- 連線在交付中斷 → CLI 報交付未確認、建議重跑 login（重跑冪等：整組覆寫）。

**驗收準則：**

- fleety-protocol：新 frame 的 serde round-trip 測試（含未知欄位容忍），`config_protocol` bump 的 Welcome 測試更新。
- fleety-cli：版本閘（<2 拒絕）、交付成功/失敗訊息、殘檔清理與提示的單元測試（連線層 mock 或沿用既有 CLI 測試姿態）。
- fleety-server：handler 的單元/整合測試——合法 put 後檔案存在且內容等於送入 Tokens、status 反映存在與過期、delete 後檔案消失、auth-off 拒絕、未知 kind 拒絕、壞 payload 拒絕且不落地；audit 事件斷言。
- 全 workspace：cargo test、cargo clippy -D warnings、cargo fmt 乾淨。
- 手動端到端（發版後）：Windows CLI 對 Mac mini server 跑 auth login，server 端出現 token 檔、`fleety auth status` 回報已登入、模型呼叫走 ChatGPT。

**範圍邊界：**

- 範圍內：crates/fleety-protocol/src/lib.rs、crates/fleety-cli/src/auth.rs、crates/fleety-tools/src/oauth.rs（僅補 Tokens serde 需要的 derive/helper，若已足夠則不動）、crates/fleety-server/src/conn.rs、docs/env.md、README.md。
- 範圍外：providers.rs 消費端、oauth PKCE 流程、keychain、多 server 同步、config 面板。

## Risks / Trade-offs

- [token 經 ws:// 明文區網傳輸] → 非新增風險：device token、config 值走同一通道；信任邊界=已配對認證的連線。TLS/wss 是獨立的既有課題，不在本變更內解。
- [PKCE 完成但交付失敗 → token 蒸發，使用者要重授權] → 可接受：重跑 login 成本低（瀏覽器已登入 ChatGPT 時幾乎即時），比「暫存本機再補送」的狀態機簡單且不留敏感殘留。login 前置版本閘已把最常見的「白跑授權」擋在開瀏覽器之前。
- [refresh 後 server 寫回檔案 vs 使用者同時 logout 的競態] → 檔案級操作本就 last-write-wins，現狀已如此；logout 後下一次 refresh 會因檔案不存在而失敗並回報未登入，行為正確。
- [CONFIG_PROTOCOL_VERSION 承載 credential 能力，語義略寬] → 以文件註解講清楚版本 2 的含義；若未來 credential 面獨立演化再拆欄位（additive，不受此決策鎖死）。

## Migration Plan

單一 change 出貨：protocol bump + server handler + CLI 改造同版釋出（binaries lockstep，fleet 收斂會把 CLI 與 server 拉到同版，跨版窗口由版本閘顧住）。使用者動作：升級後在任一裝置重跑 `fleety auth login` 一次。回滾即 revert；server 端 token 檔格式未變，回滾後同機情境自動回到舊行為。

## Open Questions

- 無阻斷項。frame 欄位命名於 apply 時對齊 conn.rs 鄰近 frame 的既有慣例。
