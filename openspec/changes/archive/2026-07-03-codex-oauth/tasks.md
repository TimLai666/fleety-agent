## 1. OAuth 核心(token store 與 PKCE)

- [x] 1.1 [P] 在 crates/fleety-tools/src/oauth.rs 實作 PKCE:高熵 code_verifier 產生與 S256 challenge 計算(base64url 無 padding)。驗證:challenge = base64url(sha256(verifier)) 的單元測試與已知向量比對。(涵蓋需求: ChatGPT login uses a PKCE authorization-code flow)
- [x] 1.2 [P] 在 crates/fleety-tools/src/oauth.rs 實作 token store 讀寫:`{ access_token, refresh_token, expires_at_secs, token_type }` 至 `~/.fleety/codex-oauth.json`,Unix 建立時 chmod 0600;不入 config/providers。驗證:寫入後往返一致、Unix 下檔案模式為 0600、損毀檔讀為「未登入」的單元測試。(涵蓋需求: OAuth tokens are stored protected and refreshed automatically)
- [x] 1.3 實作 bearer 供應者抽象:static(回固定 key)與 oauth(讀 token store、access token 到期或接近到期則以 refresh token 換新並回存;refresh 失敗回可行動錯誤)。驗證:未到期回既有、到期觸發 refresh、refresh 失敗回可行動錯誤,以可注入時鐘 + HTTP stub 驗證。(涵蓋需求: OAuth tokens are stored protected and refreshed automatically;A provider can authenticate with the OAuth token)

## 2. 登入流程(CLI)

- [x] 2.1 在 crates/fleety-cli/src/auth.rs 實作 `fleety auth login`:產 verifier/challenge/state、組授權 URL、開瀏覽器(`--no-browser` 改印 URL)、起臨時 loopback listener 接 `?code=&state=`、驗 state 後以 code+verifier 換 token 並存檔。驗證:以本機假授權/token 端點 stub 走完流程存下 token 的整合測試,及 state 不符即中止的測試。(涵蓋需求: ChatGPT login uses a PKCE authorization-code flow)
- [x] 2.2 實作 `fleety auth status`(回報登入與到期、不印 token 值)與 `fleety auth logout`(刪 token 檔);login/logout 寫入 audit。驗證:status 不外洩 token、logout 後視為未登入、audit 記一筆的測試。(涵蓋需求: Login status and logout do not leak tokens)
- [x] 2.3 在 crates/fleety-cli/src/main.rs 接上 `auth` 子命令分派(login/status/logout)。驗證:各子命令可被 dispatch 到、未知子命令回 usage 的測試。(涵蓋需求: Login status and logout do not leak tokens)

## 3. Provider 認證模式接線

- [x] 3.1 擴充 crates/agent-core/src/openai.rs 的 `OpenAiCompat`,每次請求前經 bearer 供應者取得有效 bearer(而非只用建構時的靜態 key);既有靜態 key 為預設實作、行為不變。驗證:靜態模式行為不變、oauth 模式每次取 bearer 的單元測試(以 stub 供應者)。(涵蓋需求: A provider can authenticate with the OAuth token)
- [x] 3.2 [P] 在 provider 設定新增認證模式:env `{prefix}_AUTH`(static 預設 | oauth:codex)與 providers.toml `auth` 欄位;build 時據此掛 static 或 oauth 供應者。未設維持現況。驗證:`{prefix}_AUTH=oauth:codex` 掛上 oauth 供應者、未設維持 static 的單元測試。(涵蓋需求: A provider can authenticate with the OAuth token)
- [x] 3.3 在 crates/fleety-server/src/providers.rs 的 build_provider / build_from_spec 傳入認證模式並構造對應 bearer 供應者;oauth 但未登入時模型呼叫回可行動錯誤。驗證:oauth 未登入回可行動錯誤、oauth 已登入用該 bearer 的測試。(涵蓋需求: A provider can authenticate with the OAuth token)

## 4. 設定與文件

- [x] 4.1 [P] 在 typed config registry(crates/fleety-tools/src/config.rs)新增可覆寫的 OAuth client_id、authorize endpoint、token endpoint、後端 base URL,附 Codex CLI 已知公開預設值;端點於實作時對照 Codex CLI 現況確認。驗證:config list/get 顯示新鍵與預設、覆寫生效的單元測試。(涵蓋需求: Login status and logout do not leak tokens)
- [x] 4.2 [P] 更新 docs/env.md(OAuth 設定鍵)與 README.md(`fleety auth login` 用法與 provider 設 oauth:codex 的說明),與實作一致。驗證:內容審查對照命令、設定鍵與預設。
