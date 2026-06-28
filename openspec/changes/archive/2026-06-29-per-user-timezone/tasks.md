<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。依賴 identity-core 的 acting user。 -->

## 1. 時區解析與格式化

- [x] 1.1 [P] 在 crates/fleety-server（storage.rs 或小模組）加：user profile 的 optional IANA `timezone` 欄位讀取、純函式 `resolve_tz(user_tz, env_tz) -> chrono_tz::Tz`（precedence user→FLEETY_TZ→UTC、無效值往下 fall through 不報錯）、純函式 `format_for_user(ts_secs, tz) -> String`（epoch→該時區人類字串，用 chrono-tz）——交付 "Times are presented in the acting user's timezone"、"Stored timestamps remain UTC" 的解析/格式化核心（決策「Timezone resolves acting-user, then FLEETY_TZ, then UTC」「Render-only: a format-for-user helper; storage stays UTC」）。驗證:resolve_tz（有效 user 勝、無效 user→env、無效/缺 env→UTC、Guest→env 再 UTC）與 format_for_user（已知 epoch 在 Asia/Taipei 與 UTC 的預期字串）純函式測試;cargo test -p fleety-server 綠。

## 2. 接線（現在時間注入 + 顯示）

- [x] 2.1 在 crates/fleety-server/src/conn.rs 回合開始時用 acting user 的解析時區把「目前時間」注入 prompt（讓 agent 以正確時區推理）；audit/recall/listing 等顯示時間改走 `format_for_user`；寫入時間維持 epoch（不在寫入端本地化）——交付 "Times are presented in the acting user's timezone"、"Stored timestamps remain UTC" 的接線（決策「Inject "now" in the acting user's zone at turn start」「Render-only: a format-for-user helper; storage stays UTC」）。驗證:注入的「現在時間」用解析時區、顯示走 format_for_user、儲存仍 epoch 的測試/審查;cargo build -p fleety-server 綠。

## 3. 文件

- [x] 3.1 [P] docs/env.md：記錄 per-user timezone 與 `FLEETY_TZ` fallback、儲存仍 UTC、僅呈現本地化——交付:文件與行為一致。驗證:內容審查。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*。
