## 1. 協定層（fleety-protocol）

- [x] 1.1 依 design「決策一：憑證專用 frame 組」與 spec 的 Credential delivery frames 要求，在 crates/fleety-protocol/src/lib.rs 新增 CredentialPut { kind, payload } / CredentialStatus { kind } / CredentialDelete { kind }（ClientMsg）與 CredentialResult / CredentialStatusResult（ServerMsg，status 回覆僅 presence、expiry、非敏感 detail，欄位命名對齊鄰近 frame 慣例）。先寫 serde round-trip 測試（含未知欄位容忍）再實作（tdd）。驗證：cargo test -p fleety-protocol 新增測試全綠。
- [x] 1.2 依 design「決策四：config_protocol 版本 bump 到 2」與 spec 的 Credential capability is version-negotiated 要求，把 CONFIG_PROTOCOL_VERSION 升為 2 並更新其文件註解（版本 2 = 支援 credential frames），Welcome 序列化測試同步更新。驗證：cargo test -p fleety-protocol 全綠，註解講明版本 2 含義。

## 2. server 端（fleety-server）

- [x] 2.1 依 design「決策三：server 落地沿用既有檔案與消費端」與 spec 的 Credential delivery frames 要求，在 crates/fleety-server/src/conn.rs 新增 handler：CredentialPut(kind=codex-oauth) 反序列化為 oauth::Tokens（缺欄位拒絕且不落地）後以 oauth::save_tokens 寫入 server 的 default_token_path；CredentialStatus 以 load_tokens 回報 presence 與 expires_at_secs（永不含 token 值）；CredentialDelete 走 clear_tokens；未知 kind 回指名錯誤。先寫測試再實作（tdd）：合法 put 後檔案內容等於送入 Tokens、壞 payload 拒絕且無副作用、未知 kind 拒絕、status 不含 token 值、delete 後檔案消失。驗證：cargo test -p fleety-server 全綠。
- [x] 2.2 依 design「決策六：audit 與 auth 前置」與 spec 的 Credential writes require authentication and are audited 要求：三個 credential frame 僅接受已認證連線，FLEETY_REQUIRE_AUTH=0 的 server 回「enable auth and pair」語意錯誤；接受的 put/delete 記 audit（kind＋來源裝置、無 token 值），寫檔失敗回帶 OS 原因的錯誤並記失敗事件。先寫測試：auth-off 拒絕、audit 事件斷言。驗證：cargo test -p fleety-server 全綠。

## 3. CLI 端（fleety-cli）

- [x] 3.1 依 design「決策二：PKCE 留在 CLI，tokens 經連線交付」與 spec 的 ChatGPT login uses a PKCE authorization-code flow（修改版）要求，改造 crates/fleety-cli/src/auth.rs 的 login：PKCE 流程不變，交換到 Tokens 後改送 CredentialPut 給 current profile 的 server，成功訊息指名 server（profile＋URL），CLI 不再呼叫 save_tokens；交付失敗（不可達/未配對/被拒）整個 login 失敗並附 remediation，不落地任何 token。驗證：cargo test -p fleety-cli 全綠，login 路徑無任何本機 token 寫入（以測試或 grep 斷言 save_tokens 不再被 CLI 呼叫）。
- [x] 3.2 依 design「決策四：config_protocol 版本 bump 到 2」，login 在開瀏覽器之前檢查連線 server 的 config_protocol，< 2 即報「先 fleety update 升級 server」並中止（不進 OAuth 流程）；status/logout 同樣先檢查。先寫版本閘測試（1 拒絕、2 通過）。驗證：cargo test -p fleety-cli 全綠。
- [x] 3.3 依 spec 的 Login status and logout do not leak tokens（修改版）要求，status 改查連線 server 的 CredentialStatus 並顯示登入狀態與過期時間、logout 改送 CredentialDelete 並印指名 server 的登出訊息；輸出永不含 token 值。驗證：cargo test -p fleety-cli 既有與新增測試全綠。
- [x] 3.4 依 design「決策五：CLI 殘檔遷移」：login 成功交付後若 CLI 本機存在舊 codex-oauth.json 則刪除並提示「憑證現在存放於 server」；status 發現殘檔時提示它已不再被讀取、建議重跑 login。先寫殘檔情境測試（有殘檔→login 後消失；status 提示字樣）。驗證：cargo test -p fleety-cli 全綠。

## 4. 文件

- [x] 4.1 [P] docs/env.md 的 Codex OAuth 段落改寫：講明 auth login/status/logout 作用於連線中的 server、token 存 server 端受限權限檔、舊 server 需先升級、CLI 殘檔的遷移行為；並與 fleety pair（裝置配對）作區別說明。驗證：內容審閱與 spec 用語一致。
- [x] 4.2 [P] README.md 的 auth 相關行更新（command reference 與 OAuth 說明處），指明登入結果存放於連線中的 server。驗證：內容審閱。

## 5. 整體驗證

- [x] 5.1 全 workspace 驗證：cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過；確認 OAuth tokens are stored protected and refreshed automatically 的既有行為未回歸（fleety-tools oauth 測試全綠、providers.rs 消費端未改動——git diff 斷言該檔無變更）。驗證：指令輸出乾淨。
