## Why

`fleety resume <conversation_id>` needs an id, but there is no way to list past conversations — the id only ever appears once (on `ask`/TUI completion) and is otherwise unrecoverable. A user who didn't note it down cannot discover which conversations exist to resume.

## What Changes

- 新增 wire 請求/回應：`ClientMsg::ConversationList { limit }` 與 `ServerMsg::ConversationListResult { conversations_json }`（additive、沿用 `AuditList`/`AuditListResult` 的 JSON-encoded 慣例，舊 client 忽略即可）。
- server 端新增查詢：以連線裝置解析出的 acting user（`storage.acting_for_device`）為範圍，列出其最近對話，每筆帶 `conversation_id`、`last_ts_secs`、`events`（事件數）、`preview`（首則 user 訊息裁切後的一行摘要）；裝置無 owner（guest）時退回列該裝置 legacy 目錄下的對話。單趟讀檔取得 count/last_ts/preview，避免反序列化整份訊息。
- 新增 CLI 子命令 `fleety conversations [<limit>]`：連線送出 `ConversationList`，以最近優先印出 `id`、相對最後活動時間（重用既有 `format_relative`）、preview，讓使用者找到 `fleety resume` 需要的 id。
- 更新 `print_help` 與 `ask` 的 exhaustive `ServerMsg` match arm 以納入新變體。

## Non-Goals

- 不做跨裝置／全域對話瀏覽器；範圍限定連線裝置解析出的 acting user。
- 不做關鍵字／語意搜尋或過濾（那是 agent 端 `conversation_search`/`conversation_semantic_search` 的職責，不重複）。
- 不做對話刪除、改名，也不改動 `fleety resume` 的行為或 wire 形狀。
- 不產生 LLM 標題；preview 僅取首則 user 訊息的裁切文字。
- 不改 `fleety status`（曾被列為退而求其次選項）；採用專屬命令。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `conversation-recall`: 新增一條 user-facing 需求——透過新的 wire 請求與 `fleety conversations` 命令，讓使用者發現可 resume 的對話清單（現有需求都是 agent 內部工具面向，本次補上使用者面向的發現入口）。

## Impact

Affected specs: conversation-recall

Affected code:
- Modified: crates/fleety-protocol/src/lib.rs
- Modified: crates/fleety-server/src/storage.rs
- Modified: crates/fleety-server/src/conn.rs
- Modified: crates/fleety-cli/src/main.rs