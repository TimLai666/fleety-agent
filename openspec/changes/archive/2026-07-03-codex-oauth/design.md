## Context

Fleety 的 model provider 目前只吃靜態 API key:`build_provider`(crates/fleety-server/src/providers.rs)把 `Option<String>` key 交給 `OpenAiCompat::new` / `Gemini::new` 當 bearer,key 來自 `{prefix}_KEY` 環境變數或 providers.toml 的 `key` 欄位。沒有任何 OAuth/PKCE 流程(全 repo grep 確認 oauth/pkce 皆為測試假資料)。既有 fleetyd.token 已示範「token 存於 ~/.fleety、Unix chmod 0600」的模式。既有安全原則要求憑證存 Agent 端、不外洩明文、存取留 audit。本變更要在既有 OpenAI 相容呼叫路徑上,讓 provider 改以「可更新的 OAuth access token」作為 bearer,並補上取得/保存/更新該 token 的流程。

## Goals / Non-Goals

**Goals:**

- 一條 PKCE 授權碼登入流程(`fleety auth login`),桌機瀏覽器 + loopback 接碼。
- token 持久化(access/refresh/expiry)於受限權限檔,並在到期時自動 refresh。
- 讓被設定為 OAuth 的 provider 以 token store 的 access token 作 bearer,沿用既有呼叫路徑。
- 登入狀態檢視與登出,狀態不外洩 token 明文。
- 設定面可覆寫 client_id / 端點 / 後端 base URL,預設用 Codex CLI 已知公開值。

**Non-Goals:**

- keychain / OS secret manager 整合(沿用受限權限檔;keychain 為獨立後續)。
- device-code(無瀏覽器)流程;非 OpenAI 的通用 OAuth2 框架;帳號/計費管理。

## Decisions

**D1 provider 認證改為可注入的 token 來源。** 擴充 `OpenAiCompat`,除既有靜態 key 外,支援一個「bearer 供應者」抽象:每次請求前取得一個有效 bearer。靜態 key 是其中一種實作(回傳固定字串);OAuth 是另一種(讀 token store、必要時 refresh)。捨棄「新增獨立 provider 型別」——重用既有 request/SSE/重試路徑成本最低、行為最一致。

**D2 認證模式由設定選定。** provider 設定新增認證模式:預設 `static`(現況,用 key);設為 `oauth:codex` 時 build 時掛上 OAuth bearer 供應者。env 路徑用 `{prefix}_AUTH`,providers.toml 用 `auth` 欄位。未設即維持現況、完全相容。

**D3 PKCE 授權碼流程 + loopback 接碼。** `fleety auth login`:產生 code_verifier(高熵隨機)與 S256 challenge,開瀏覽器到授權端點(client_id、redirect_uri=http://127.0.0.1:<臨時port>/callback、scope、state、code_challenge),本機起臨時 HTTP listener 接 `?code=&state=`,驗 state 後以 code + verifier 向 token 端點換 access/refresh token。捨棄 device-code:桌機 loopback 是標準且體驗較好;headless 另議。

**D4 token store = 受限權限檔。** 存於 `~/.fleety/codex-oauth.json`,內容 `{ access_token, refresh_token, expires_at_secs, token_type }`,Unix 下 chmod 0600(比照 fleetyd.token);Windows 依既有慣例。讀寫封裝在 crates/fleety-tools/src/oauth.rs。捨棄明文進 config.toml/providers.toml(憑證不入一般設定)。

**D5 自動 refresh。** bearer 供應者在回傳前檢查 `expires_at_secs`(留安全邊際,如提前 60s 視為到期),到期則以 refresh_token 向 token 端點換新並回存;refresh 失敗回可行動錯誤(「請執行 fleety auth login 重新登入」)。呼叫端(model call)拿到錯誤即為值、不崩潰。

**D6 端點與 client_id 可設定、附已知預設。** 授權端點 / token 端點 / client_id / 後端 base URL 皆可經設定覆寫,預設採 Codex CLI 已知公開值。實際值於實作時對照 Codex CLI 現況確認(見 Open Questions);設計不綁死單一常數,以利日後端點調整。

## Implementation Contract

**行為(Behavior):** 使用者執行 `fleety auth login`,瀏覽器開啟 ChatGPT 授權頁,授權後 CLI 顯示登入成功;此後把某 provider 的認證模式設為 `oauth:codex`,Fleety 呼叫模型時以該帳號的 OAuth token 作 bearer、免 API key;token 到期自動 refresh;`fleety auth status` 顯示已登入與到期時間(不印 token),`fleety auth logout` 清除本機 token。

**介面 / 資料形狀(Interface / data shape):**

- CLI:`fleety auth login`(可選 `--no-browser` 只印 URL 供手動開)、`fleety auth status`、`fleety auth logout`。
- token store 檔:`~/.fleety/codex-oauth.json` = `{ access_token: String, refresh_token: String, expires_at_secs: u64, token_type: String }`,Unix 0600。
- provider 認證欄位:env `{prefix}_AUTH`(值 `static`(預設)|`oauth:codex`);providers.toml provider 條目 `auth`(同值集,省略即 static)。
- bearer 供應者抽象:一個非同步取得有效 bearer 的介面,OpenAiCompat 每次請求前呼叫;static 實作回固定 key,oauth 實作讀檔 + 必要時 refresh。
- 設定鍵(可覆寫、附已知預設):OAuth client_id、authorize endpoint、token endpoint、後端 base URL,納入 typed config registry。

**失敗模式(Failure modes):** 未登入卻用 oauth provider → 呼叫回「未登入,請 fleety auth login」可行動錯誤,不崩潰。refresh 失敗(refresh token 失效)→ 回可行動錯誤要求重新登入。登入流程中 state 不符 → 中止並回錯誤、不換 token。loopback 埠被占用 → 換埠重試或回可行動錯誤。token 檔損毀 → 視為未登入並提示重新登入。

**驗收(Acceptance criteria):**

- 單元測試:PKCE code_verifier/challenge(S256)產生與驗證;token store 讀寫往返 + 權限(Unix 下檔案模式)測試;bearer 供應者「未到期回既有、到期觸發 refresh、refresh 失敗回可行動錯誤」以可注入的時鐘與 HTTP stub 驗證;`{prefix}_AUTH=oauth:codex` 時 build_provider 掛上 oauth 供應者、預設 static 維持現況的測試。
- 端到端(可離線):以本機假授權/ token 端點 stub 走完 login → 存 token → provider 取 bearer → refresh 一輪;`auth status` 不外洩 token、`auth logout` 清檔。
- 相容:未設 `{prefix}_AUTH` 的既有部署行為不變(靜態 key 路徑)。

**範圍邊界(Scope boundaries):** In scope = login(loopback PKCE)、token store(0600)、refresh、provider oauth bearer 供應者與設定、status/logout、設定鍵、docs。Out of scope = keychain 整合、device-code 流程、非 OpenAI 通用 OAuth 框架、帳號/計費管理。

## Risks / Trade-offs

- [token 明文落地(非 keychain)] → 受限權限檔(0600)+ 不入一般 config;keychain 列為獨立後續(與現況既有 token 同等級,不擴張本次範圍)。
- [ChatGPT/Codex 端點或 client_id 日後變動] → 端點與 client_id 皆可設定覆寫、不綁死常數;預設值於實作時對照 Codex CLI 現況。
- [refresh token 外洩風險] → 僅存本機受限檔、status 不外洩、logout 可清;audit 記錄登入/登出事件。
- [loopback 流程在無桌面環境不可用] → 提供 `--no-browser` 印 URL;device-code 列後續。
- [第三方端點回非預期形狀] → 解析防禦性、錯誤即值回可行動訊息,永不崩潰。

## Migration Plan

- 純新增且預設相容:未設認證模式的 provider 維持靜態 key 路徑;無 token 檔即視為未登入。
- 部署:使用者顯式 `fleety auth login` 並把 provider 設為 `oauth:codex` 才生效。
- 回退:`fleety auth logout` 清 token、把 provider 認證模式改回 static(或用回 key)即恢復原狀。

## Open Questions

- ChatGPT/Codex 的實際 authorize / token 端點、公開 client_id 與後端 base URL 現值,需於實作時對照 Codex CLI 現況確認並填入預設(設計已保證可設定覆寫,不綁死)。
- scope 字串與是否需要額外 headers(如 originator)由實作對照現況決定。
