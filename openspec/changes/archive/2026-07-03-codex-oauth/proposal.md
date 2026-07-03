## Why

Fleety 目前只能用靜態 API key 接 OpenAI 相容端點。使用者要能用 ChatGPT 訂閱、透過 OAuth 登入(免自備 API key)驅動 Fleety,方向對齊 Codex CLI 的 ChatGPT 登入。這需要一條 PKCE 授權碼流程、token 的持久化與自動更新,以及讓既有 provider 能以 OAuth token(而非靜態 key)作為 bearer。先前此項標記為延後,使用者已明確要求不再延後。

## What Changes

- **新增 `fleety auth login`(PKCE 授權碼流程)**:產生 PKCE code_verifier/challenge,開瀏覽器到授權端點(client_id + PKCE + loopback redirect),以本機臨時 HTTP listener 接住授權碼,向 token 端點以 code + verifier 換取 access/refresh token。
- **OAuth token store(持久化 + 權限保護)**:access token、refresh token、到期時間存於 Agent 端專屬檔(Unix chmod 0600),比照既有 fleetyd.token 的保護方式。
- **自動更新(refresh)**:呼叫模型前若 access token 已到期,以 refresh token 向 token 端點換新;失敗回可行動錯誤(提示重新登入),絕不崩潰。
- **provider 認證模式**:讓被設定為 OAuth 的 provider 以 OAuth token store 取得 bearer(取代靜態 key),沿用既有 OpenAI 相容 provider 呼叫路徑,指向 ChatGPT/Codex 後端 base URL。
- **`fleety auth status` / `fleety auth logout`**:檢視目前登入狀態(不外洩 token 明文)與清除本機 token。
- **設定面**:新增選擇 OAuth 認證與(可覆寫的)client_id / 授權端點 / token 端點 / 後端 base URL 的設定鍵,預設採 Codex CLI 已知公開值。

## Non-Goals (optional)

- keychain / OS secret manager 整合:目前全專案尚無此整合(既有 token 亦為受限權限檔),本次沿用同模式,keychain 列為獨立的跨領域後續 change,不在此擴張。
- device-code(無瀏覽器)登入流程:桌機優先採 loopback 授權碼流程;headless device-code 列為後續。
- 通用可設定 OAuth2 provider 框架(非 OpenAI):明確不做,聚焦 ChatGPT/Codex。
- 管理 ChatGPT 帳號本身、計費或訂閱層級判斷。

## Capabilities

### New Capabilities

- `codex-oauth`: 以 ChatGPT 訂閱 OAuth(PKCE 授權碼流程)登入,持久化與自動更新 token,並讓被設定為 OAuth 的 provider 以該 token 作為 bearer 呼叫後端。

### Modified Capabilities

(none)

## Impact

- Affected specs: codex-oauth(新增)
- Affected code:
  - New:
    - crates/fleety-tools/src/oauth.rs
    - crates/fleety-cli/src/auth.rs
  - Modified:
    - crates/fleety-cli/src/main.rs
    - crates/agent-core/src/openai.rs
    - crates/fleety-server/src/providers.rs
    - crates/fleety-tools/src/config.rs
    - crates/fleety-tools/src/providers_config.rs
    - docs/env.md
    - README.md
  - Removed: (none)
- Dependencies: PKCE 需要一個 loopback HTTP listener 與 SHA-256/base64url(既有相依已可涵蓋雜湊與 base64;HTTP client 用既有 reqwest;loopback 監聽用既有 async runtime)。目標盡量零新增外部 crate;若確需極小相依,於 design 標明並說明理由。
