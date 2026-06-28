<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機 LLM 真的會去 fetch 為環境相依，需手動驗證。 -->

## 1. agent-core：可定位的截斷標記

- [x] 1.1 在 crates/agent-core/src/compress.rs 讓 `budget_text` 與 `compress_tool_result` 收 tool-call id，截斷時 marker 改成「`...[truncated N chars; fetch the full result with fetch_tool_result id="<id>"]`」；無 id 時退回原措辭、未截斷不變；並在 crates/agent-core/src/agent.rs 餵工具結果時把該次 ToolCall 的 id 傳入——交付 "Truncated tool results are locatable"（決策「The truncation marker carries the tool-call id (no new id invented)」）。驗證:單元測試「有 id→marker 含 id」「無 id→原措辭」「未截斷→無 marker」;既有 compress 測試仍綠;cargo test -p agent-core 全綠;`cargo tree -p agent-core` 無 fleety-*。

## 2. server：分段、預算化、user-scoped 取回

- [x] 2.1 在 crates/fleety-server/src/storage.rs 加「依 id 在 acting user 可存取的對話內解析全量 tool result」：記錄 tool-result 事件時標上 conversation_id，提供 `tool_result_for(user, conversation_hint, id) -> Option<(value, conversation_id)>`（先當前對話、再該 user scope 內最近；跨 user 視為找不到）——交付 "Full tool results are retrievable in bounded segments" 的解析面與 "Tool-result retrieval and audit listing respect the user boundary" 的範圍面（決策「Retrieval source = the event log, scoped to the acting user's conversations」）。驗證:單元測試（命中當前對話、跨 user→None、缺→None、壞行跳過）;cargo test -p fleety-server 綠。
- [x] 2.2 在 crates/fleety-server/src/tools.rs 新增 `fetch_tool_result(id, offset?, limit?)`（risk=Read）：回 `{ id, total_chars, offset, returned_chars, next_offset, content }`，content 為字元窗 `offset..offset+limit`，limit 預設且上限為 tool-result 預算，offset 超界→空+next_offset=null；非字串值先正規化成字串再切窗——交付 "Full tool results are retrievable in bounded segments"（決策「`fetch_tool_result(id, offset?, limit?)` returns bounded, budgeted segments」）。驗證:單元測試（依序 paging 到 null 取得全量、limit 夾到預算、offset 超界空、未知 id→not found）;cargo test -p fleety-server 綠。
- [x] 2.3 在 crates/fleety-server/src/tools.rs 讓 `history_list` 依 acting user 過濾（只回該 user 可存取對話的 entries），並在 crates/fleety-server/src/conn.rs 以 acting user 註冊 `fetch_tool_result`、記錄 tool-result 事件時帶 conversation_id——交付 "Tool-result retrieval and audit listing respect the user boundary"（決策「`history_list` gains the same acting-user filter」）。驗證:隱私測試「A 不能 fetch B 對話的 id（not found、無存在提示）」「history_list 只回 acting user 的 entries」（延伸 privacy-isolation 測試）;cargo build -p fleety-server 綠;實機 LLM 自發 fetch 標手動驗證。

## 3. 文件

- [x] 3.1 [P] docs/env.md：說明截斷可帶 id、`fetch_tool_result` 分段＋預算＋user-scoped 取回、`history_list` 已按 acting user 過濾（event log 仍為真相）——交付:文件與行為一致。驗證:內容審查。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機 LLM 取回需手動驗證。
