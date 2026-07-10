## Why

一組面向使用者的 CLI/ACP 打磨缺陷：配對失敗把內部型別的 Debug 字串倒給使用者、OAuth 登入在固定 loopback port 被占用時體驗差、install.sh 的安裝目錄判斷與實際權限不符、TUI 貼上檔案無大小上限且把原始碼一律標成 `text/plain`、ACP `session/load` 回空物件可能不符規範。逐項修成乾淨、可讀、符合契約的行為。

## What Changes

- `fleety pair` 收到非 `Welcome`/`Error` 的意外回覆時，改印簡潔可讀訊息，不再輸出 `{other:?}` 這種內部型別 Debug 傾印（`crates/fleety-cli/src/main.rs`）。
- `fleety auth login` 在開瀏覽器前先偵測固定 loopback port 是否可用；被占用時提前中止並給出「此 port 為註冊死綁、如何釋放」的可操作指引，避免使用者跑完授權才在 redirect 失敗（`crates/fleety-cli/src/auth.rs`）。
- `scripts/install.sh` 以實際寫入探測（建立再刪除暫存檔）取代 `[ -w /usr/local/bin ]`，避免對不存在或 root 擁有的目錄誤判；選定目錄不在 `PATH` 時一律警告並印出加入方式。
- TUI 貼上檔案：加入最大附件位元組上限，超過上限不再靜默全量讀入 base64，改回退成把路徑當文字插入；原始碼副檔名（`.rs`/`.py`/`.js`/`.ts`/`.go` 等）帶可辨識語言的 text MIME 並保留原始檔名，未知型別維持 `application/octet-stream`（`crates/fleety-cli/src/clipboard.rs`）。
- ACP `session/load` 在歷史 replay 後，回傳依 ACP `LoadSessionResponse` 形狀構造的良好回應，取代碰巧能反序列化的空物件（`crates/fleety-cli/src/acp.rs`）。

## Non-Goals

- 不改變 OAuth 固定 loopback port 本身（redirect URI 已註冊死綁），只改偵測與訊息。
- 不動 `scripts/install.ps1`（Windows 安裝腳本）；本次僅修 `install.sh`。
- 不新增可由環境變數調整的剪貼簿大小上限；先用編譯期常數與合理預設。
- 不重構 ACP 傳輸層、session 模型或既有 `session/prompt`、`session/cancel` 行為。
- 不新增 provider 認證模式或改動 token 儲存。

## Capabilities

### New Capabilities

- `clipboard-paste`: 定義 TUI 貼上（Ctrl+V）把剪貼簿內容轉為訊息附件的行為契約——附件大小上限與超限回退、以及附件型別可被 server 辨識。

### Modified Capabilities

- `device-enrollment`: 新增「配對失敗回可讀訊息」需求。
- `codex-oauth`: 新增「登入在 loopback port 不可用時提前失敗」需求。
- `acp-adapter`: 新增「session/load 回符合規範的回應」需求。
- `self-update`: 修改既有「Sidecar and install paths」需求的安裝目錄判斷條款。

## Impact

- Affected specs: clipboard-paste (new), device-enrollment, codex-oauth, acp-adapter, self-update
- Affected code:
  - Modified: crates/fleety-cli/src/main.rs
  - Modified: crates/fleety-cli/src/auth.rs
  - Modified: crates/fleety-cli/src/clipboard.rs
  - Modified: crates/fleety-cli/src/acp.rs
  - Modified: scripts/install.sh
