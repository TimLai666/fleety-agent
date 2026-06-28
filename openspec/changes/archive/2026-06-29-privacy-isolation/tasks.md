<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機跨裝置/跨人實資料行為為環境相依，需手動驗證。依賴 identity-core 的 ActingUser。 -->

## 1. 存取守衛（privacy.rs）

- [x] 1.1 [P] 新增 crates/fleety-server/src/privacy.rs：純函式 `can_access(acting: &ActingUser, resource_owner, grants) -> Decision`（acting==owner→Allow；有涵蓋的 grant→Allow；否則 Deny；Guest→Deny 所有私有；錯誤一律 fail closed）＋ `Grants` 明確授權儲存（owner→{grantee,scope}）load/save——交付 "The acting user is a hard privacy boundary"、"Cross-user access requires an explicit grant" 的判定核心（決策「A data-layer access guard keyed to the acting user」「Cross-user access is default-deny with explicit, coarse grants」「Guest gets no private data」）。驗證:純函式測試 owner→Allow、無 grant 他人→Deny、grant 內→Allow、grant 外→Deny、Guest→Deny、壞 grants→Deny;cargo test -p fleety-server 綠。

## 2. 存放重排與遷移

- [x] 2.1 在 crates/fleety-server/src/storage.rs 把對話改為 user-primary：路徑 `users/<user_id>/conversations/<id>.jsonl`、每個事件記 `device_id`、維護 `conversation_id → owner` 索引、所有對話/記憶/recall 存取改為「吃 &ActingUser、只回該人(或被 grant)的資料」（無未受 scope 的 turn 讀取路徑）——交付 "Conversations are stored per user, with device recorded, migrated losslessly"、"The acting user is a hard privacy boundary" 的存放面（決策「User-primary conversation storage, device recorded per event」「A data-layer access guard keyed to the acting user」）。驗證:user A 讀只得 A、跨人嘗試走守衛被 Deny、id→owner 索引解析 resume 的單元測試;cargo test -p fleety-server 綠。
- [x] 2.2 在 storage.rs 實作一次性遷移 `migrate_conversations()`：把既有 `devices/<device>/conversations/*` 依 device.owner 移到該 user（無 owner→保留 unattributed bucket）、每事件補 device_id、建 id→owner 索引；idempotent、verify-before-delete（崩潰不丟資料）——交付 "Conversations are stored per user, with device recorded, migrated losslessly" 的遷移面（決策「One-time migration with an id→owner index」）。驗證:有 owner→移至該人並記 device_id、無 owner→unattributed、idempotent 重跑 no-op、resume by id 仍可、source 驗證後才刪的單元測試;實機大量資料標手動驗證。

## 3. 接線與政策

- [x] 3.1 在 crates/fleety-server/src/conn.rs 把 identity-core 的 acting_user 傳入每個對話/記憶/recall 存取；Deny 映射為**統一、不洩漏**回應（不分「無此資料」與「存在但禁止」）——交付 "No disclosure of another user's content, timing, or existence" 的執行面（決策「A data-layer access guard keyed to the acting user」「No-leak covers content, timing, and existence — and refusals reveal nothing」）。驗證:跨人讀回統一非揭露回應（absent 與 forbidden 不可分）的測試;cargo build -p fleety-server 綠。
- [x] 3.2 [P] prompts/policy.md：加 no-leak 硬規則——未經本人授權絕不揭露另一使用者的內容/使用時間/是否存在或聊過；跨人需明確授權；拒絕用統一非揭露措辭——交付 "No disclosure of another user's content, timing, or existence"（決策「No-leak covers content, timing, and existence — and refusals reveal nothing」）。驗證:內容審查（涵蓋內容/時間/存在三者 + 統一拒絕）。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機跨裝置/跨人與遷移需手動驗證。
