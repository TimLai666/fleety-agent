<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機 spawn + recall 子對話為環境相依，需手動驗證。 -->

## 1. 子對話身分與父→子連結（storage，純/可測）

- [x] 1.1 [P] 在 crates/fleety-server/src/storage.rs 加：由 task id 推導穩定子對話 id 的函式（`sub-<task_id>`）＋ 父→子連結索引 `subagent_link(parent, child)` / `subagent_children(parent) -> Vec<String>`（存 `fleet/` 下的小 json，dedup）——交付 "The parent conversation links to its subagent children" 的索引核心（決策「A subagent run is a child conversation, id derived from its task id」「The parent records an explicit link to the child」「Reuse existing storage machinery」）。驗證:單元測試（id 推導決定性、link 加入/列出/dedup round-trip）;cargo test -p fleety-server 綠。

## 2. 子對話歸屬 + 事件標記（subagent host）

- [x] 2.1 在 crates/fleety-server/src/subagent.rs 讓 host 取得「父回合 acting user」（接線改：由 conn 傳入，取代以 `acting_for_device` 當擁有者）、用它 `register_conversation_owner(child_id, acting)`；guest 則不歸屬——交付 "A subagent run is a retrievable, user-owned child conversation" 的歸屬面（決策「The child conversation is owned by the parent's acting user」）。驗證:測試「host 以傳入的 parent acting user 註冊 child owner；guest→不歸屬」（以可注入 storage）;cargo test -p fleety-server 綠;實機標手動驗證。
- [x] 2.2 在 crates/fleety-server/src/subagent.rs 讓 `record_events` 改用 `append_history_tagged(device_id, child_id, ev)`（不再 untagged），並把 subagent 的 transcript 以 child_id 存成可檢索對話——交付 "A subagent run is a retrievable, user-owned child conversation" 與 "The host records a subagent under a parent-owned child conversation"（決策「Events are tagged to the child conversation」「Reuse existing storage machinery」）。驗證:測試「subagent 事件以 child_id tagged 寫入（可由 tool_result_for 取回該 child 範圍）」;cargo test -p fleety-server 綠;實機 recall 子對話標手動驗證。

## 3. 父端連結接線（conn）

- [x] 3.1 在 crates/fleety-server/src/conn.rs 把父回合 acting user 傳入 subagent host；spawn 時在父對話記 `spawn→child_id` 連結、完成 seed 標明 child_id；並寫入 1.1 的父→子索引——交付 "The parent conversation links to its subagent children"（決策「The parent records an explicit link to the child」）。驗證:測試「spawn 結果含 child_conversation_id；父→子索引記到該 child」;cargo build -p fleety-server 綠;實機標手動驗證。

## 4. 文件

- [x] 4.1 [P] docs/env.md：說明 subagent 現以「父使用者擁有的子對話」儲存（child id 由 task id 推導、事件 tagged、可 recall/可 fetch_tool_result）、父對話有 spawn→child 連結與父→子索引、guest 不歸屬——交付:文件與行為一致。驗證:內容審查。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*（subagent 機制在核心、本變更只動 server 端持久化）;記錄實機 spawn+recall 子對話需手動驗證。
