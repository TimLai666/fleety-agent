<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。 -->

## 1. 身分核心（解析 + 儲存）

- [x] 1.1 [P] 新增 crates/fleety-server/src/identity.rs：`enum ActingUser { User(String), Guest }` 與純函式 `resolve_acting_user(device_owner, device_users, asserted) -> ActingUser`（asserted 有效且被授權→User；否則 owner→User；否則 Guest；shared 裝置上 asserted 不在 users 內→Guest 失敗關閉）＋ user_id slug 驗證（比照 validate_id，禁 / 與 :）——交付 "Each turn resolves an acting user"（決策「acting_user resolution is pure and layered on the device token」「Guest is a first-class principal」）。驗證:純函式單元測試涵蓋 Example 表四列（owner、asserted-authorized、asserted-unauthorized→Guest、none→Guest）+ slug 驗證;cargo test -p fleety-server 綠。
- [x] 1.2 在 crates/fleety-server/src/storage.rs：新增 `users/<id>/` 每人 profile（USER.md）讀寫 + users 索引 + `user_profile(acting) -> String`（Guest→中性 placeholder、無個資；缺檔→建 DEFAULT_USER）；並在 device 記錄/ensure_device 加 additive `owner: Option<String>`、`users: Vec<String>`、`shared: bool`（serde default；舊 device.json 無欄位→預設）——交付 "Devices record ownership"、"The agent's user profile is the acting user's" 的儲存面（決策「Per-user identity store: `fleet/users/<user_id>/` + a users index」「Device ownership: `owner` / `users` / `shared` on device.json」）。驗證:users/<id>/USER.md round-trip、index 列出、Guest profile 中性無個資、舊 device.json 載入得預設、ensure_device 寫 owner/users/shared 的單元測試;cargo test -p fleety-server 綠。

## 2. 協定與接線

- [x] 2.1 [P] 在 crates/fleety-protocol/src/lib.rs 於使用者訊息加 additive optional acting-user 主張欄位（serde default/skip、不升 PROTOCOL_VERSION、向後相容；僅識別不授權）——交付 "The acting-user assertion is additive and backward compatible"（決策「acting_user assertion rides an additive, optional wire field」）。驗證:有/無該欄位的序列化 round-trip + 舊訊息仍可解析、版本不變;cargo test -p fleety-protocol 綠。
- [x] 2.2 在 crates/fleety-server/src/conn.rs（與 auth.rs）每個 turn 用 1.1 解析 acting_user（從連線的 device 記錄 owner/users + 2.1 的主張欄位），attach 到 turn 供後續（privacy-isolation）scope；core_memory 的 USER 區塊改用 `user_profile(acting_user)`（ME/TODO 維持全域）；per-device token 不變——交付 "Each turn resolves an acting user"、"The agent's user profile is the acting user's" 的接線（決策「acting_user resolution is pure and layered on the device token」「Core-memory USER block = the acting user's profile」）。驗證:core_memory 在指定 acting user 時注入該人 profile、Guest 時為中性、ME/TODO 全域不變的測試;cargo build -p fleety-server 綠。

## 3. 文件

- [x] 3.1 [P] prompts/memory.md：說明 USER 區塊是「當前 acting user」的 profile（per-user）、ME/TODO 為 agent 全域、Guest 為中性無個資——交付:文件與行為一致。驗證:內容審查。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*。
