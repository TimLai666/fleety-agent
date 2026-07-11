## Why

`fleety auth login` 目前把 ChatGPT（Codex OAuth）憑證存在**執行 CLI 的那台機器**的 `~/.fleety/codex-oauth.json`，但憑證的消費者是 fleety-server（provider `oauth:codex` 時由 server 讀它自己本機的 token 檔打模型）。CLI 與 server 不同機時，登入結果對 server 毫無作用，使用者被誤導以為完成了設定。這與兩個既定架構相悖：「所有 node 憑證一律存 Agent（server）端 secret store」的拍板決策，以及 config 面已完成的遠端化（`fleety config` 的 Server 區域經連線直接改 server）。CLI 的定位是遠端控制 server 的介面，auth 卻是唯一還在寫本機檔案的設定動作。

## What Changes

- `fleety auth login`：PKCE 瀏覽器流程與 loopback 回調**留在 CLI 端**（授權頁必須開在使用者面前），交換到 tokens 後改為經**已配對認證的 WebSocket 連線**交付給當前連線的 server；server 寫入它自己的 `~/.fleety/codex-oauth.json`（沿用受限權限與既有消費端），CLI 端不落地任何 token 檔。成功訊息指名憑證存到了哪個 server。
- `fleety auth status` / `fleety auth logout`：改為查詢／清除**連線中 server** 的憑證狀態（不再讀寫本機檔）；status 回報不含 token 值，只含登入狀態與過期時間（沿用現行遮蔽原則）。
- fleety-protocol 新增憑證交付 frame 組（put / status / delete，kind 先只有 codex-oauth，形狀預留其他憑證種類），沿用 config protocol 的能力版本協商模式：舊 server 不支援時 CLI 明確報「server 版本過舊」，不靜默降級回本機儲存。
- server 端新增 frame handler：驗 payload 形狀、寫入／刪除 token 檔、每次憑證寫入與清除記 audit；依 auth-default-on 既有原則，憑證寫入屬 mutating 動作，auth 關閉的 server 拒絕遠端憑證寫入。
- 遷移與清理：CLI 端執行 auth 子指令時若發現本機殘留舊版 `codex-oauth.json`，login 成功交付後刪除本機檔並提示；`auth status` 發現殘檔時提示它已不再被使用。server 端既有檔案格式不變，refresh 邏輯（本來就在 server 端消費時做）不動。
- docs/env.md 與 README 的 auth 段落改寫：講明「登入的是連線中的 server」、與 `fleety pair`（裝置配對）的區別。

## Non-Goals

- 不動 OAuth PKCE 流程本身（authorize URL、固定 loopback port 1455、token exchange 全部照舊）。
- 不動 server 端消費鏈（`OAuthCodexAuth` 讀 server 本機檔、自動 refresh 照舊）。
- 不做通用 secret manager／keychain 整合（受限權限檔案維持現狀，keychain 列為既有後續項）。
- 不做多 server 憑證同步（憑證屬於你登入時連線的那個 server；換 profile 要在新 server 重新 login）。
- 不提供 `--local` 本機儲存逃生口（同機情境連 localhost server 走同一條路即可，不留分岔）。

## Capabilities

### New Capabilities

- `server-credential-store`: 經已認證連線把節點憑證交付給 server 儲存／查詢／清除的協定與儲存行為（首個 kind：codex-oauth），含 audit 與 auth 要求。

### Modified Capabilities

- `codex-oauth`: 登入產出的憑證儲存位置從 CLI 本機檔改為交付連線中的 server；status/logout 遠端化；本機殘檔遷移提示。

## Impact

- Affected specs: `server-credential-store`（新增）、`codex-oauth`（修改）
- Affected code:
  - Modified:
    - crates/fleety-protocol/src/lib.rs
    - crates/fleety-cli/src/auth.rs
    - crates/fleety-tools/src/oauth.rs
    - crates/fleety-server/src/conn.rs
    - docs/env.md
    - README.md
  - New: （無新檔案）
  - Removed: （無）
- 相容性：舊 CLI 對新 server 不受影響（新 frame 是 additive）；新 CLI 對舊 server 在 auth 子指令明確報版本不足，其他功能不受影響。wire 變更走既有能力版本協商模式，不破壞現有 frame。
- 安全：token 經已配對認證的 WS 傳輸（與 device token、config 寫入同一信任邊界）；server 端寫入記 audit；status 永不回傳 token 值。
