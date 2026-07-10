## 1. Wire frames

- [x] 1.1 在 `crates/fleety-protocol/src/lib.rs` 新增 `ClientMsg::ConversationList { limit: Option<u32> }` 與 `ServerMsg::ConversationListResult { conversations_json: String }`（`limit` 用 `#[serde(default, skip_serializing_if = "Option::is_none")]`），交付「使用者可列出可 resume 對話」所需的 additive 契約；驗證：新增 `cargo test -p fleety-protocol` 測試 `conversation_list_frames_roundtrip`，涵蓋含/省略 `limit` 的序列化 round-trip 與舊 frame（缺欄位）仍可解析。

## 2. Server query

- [x] 2.1 [P] 在 `crates/fleety-server/src/storage.rs` 新增 `recent_conversations(&self, device_id: &str, limit: usize) -> Vec<serde_json::Value>`：以 `acting_for_device` 解析 owner（有則讀 `fleet/users/<owner>/conversations`，guest/無 owner 則讀 `fleet/devices/<device_id>/conversations`），單趟讀每檔取得 `events`、`last_ts_secs` 與 `preview`（首則 role==user 訊息 `content` 裁切為單行、上限約 80 字），依 `last_ts_secs` 由新到舊排序並截斷至 `limit`；驗證：`cargo test -p fleety-server` 新增測試證明「listing is scoped to the acting user」（他人對話不出現）、newest-first、preview 取自首則 user 訊息、guest 退回 device 目錄、`limit` 截斷。\n- [ ] 2.2 [P] 在 `crates/fleety-server/src/conn.rs` 新增 `ClientMsg::ConversationList { limit }` 分支：以連線 `device_id` 呼叫 `storage.recent_conversations`（`limit` clamp 1..=50、預設 20），回 `ServerMsg::ConversationListResult { conversations_json }`（`serde_json::to_string` 失敗退 `"[]"`），失敗以 errors-as-messages 回 `ServerMsg::Error`；owner 無對話時回空陣列而非錯誤，支撐「empty listing is honest」；驗證：`cargo test -p fleety-server` 並以既有 conn 測試風格加一則「ConversationList 回 ConversationListResult 且只含 owner 對話」的案例。

## 3. CLI command

- [x] 3.1 [P] 在 `crates/fleety-cli/src/main.rs` 新增 `conversations` 子命令（`fleety conversations [<limit>]`）：`connect_hello` 後送 `ConversationList`，解析 `conversations_json`，以最近優先印出 `conversation_id`、`format_relative(now, last_ts_secs)`、`preview`，空清單印 `(no conversations)`；同步更新 `print_help` 條列與 `ask` 中 exhaustive `ServerMsg` match arm 納入 `ConversationListResult`，落實「discovery feeds resume」的使用者入口，對應 spec requirement「Users can discover resumable conversations from the CLI」；驗證：`cargo build -p fleety-cli` 通過、`cargo test -p fleety-cli` 對 preview 裁切輔助函式加單元測試，並手動對執行中的 server 跑 `fleety conversations` 確認輸出可用、取其 id 餵 `fleety resume <id>` 能 replay。

## 4. Docs and verification

- [x] 4.1 更新 `crates/fleety-cli/src/main.rs` 的 help 文字與 README 命令參考（若有）描述 `conversations`；驗證：`fleety help` 內容審閱含新命令一行說明，且 workspace `cargo test` 全綠。