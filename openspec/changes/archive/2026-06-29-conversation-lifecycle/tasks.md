<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機端到端（真實 goal 完成→隱式蒸餾+rollover 跨模型回合）為環境相依，需手動驗證。 -->

## 1. 協定與儲存

- [x] 1.1 [P] 在 crates/fleety-protocol/src/lib.rs 新增 additive `ServerMsg::ConversationRolled { old, new }`（serde skip/default、不升 PROTOCOL_VERSION、向後相容）——交付 "Conversations can roll over without losing history" 的線協定面（決策「Client is told via an additive `ConversationRolled` message, with transparent redirect as fallback」）。驗證:序列化/反序列化 round-trip 測試 + 舊訊息流（無此欄位）仍可解析;cargo test -p fleety-protocol 綠。
- [x] 1.2 [P] 在 crates/fleety-server/src/storage.rs 讓對話可標記 ended 並記 successor conversation id，並提供「解析某對話的 active successor（鏈式）」存取——交付 "Conversations can roll over without losing history" 的持久化面（決策「Rollover mints a successor; old conversation is preserved and chained」）。驗證:標記 ended+successor 持久化、resolve active successor 鏈式正確的單元測試（舊對話仍可 load→可被 recall）;cargo test -p fleety-server 綠。

## 2. Rollover 與觸發

- [x] 2.1 在 crates/fleety-server/src/tools.rs 註冊 `rollover_conversation { distill?, note? }` 工具，並在 conn.rs 實作 rollover 處理：mint successor、切換該連線的 active conversation、emit ConversationRolled、對忽略該訊息仍送舊 id 的 client 做 transparent redirect 到 successor——交付 "Conversations can roll over without losing history"、"Rollover is agent-judged, triggered explicitly or by a silent nudge" 的顯式觸發與切換（決策「Rollover mints a successor; old conversation is preserved and chained」「Client is told via an additive `ConversationRolled` message, with transparent redirect as fallback」「Two triggers, both agent-judged; the implicit one is silent」）。驗證:工具 mint 新 id+記 successor+回新 id、舊對話仍可 load;transparent redirect 解析測試;cargo build -p fleety-server 綠。
- [x] 2.2 在 crates/fleety-server/src/conn.rs 加「隱式 lifecycle 反思」：goal 完成後（與 context 壓力高時）以 out-of-band 反思（沿用 learning-loop 的 maybe_reflect 靜默路徑）提示 agent 判斷是否蒸餾+rollover；反思輸出**不**以 user-facing assistant turn 送出（靜默）；長度只 nudge 不強切——交付 "Rollover is agent-judged, triggered explicitly or by a silent nudge"（決策「Two triggers, both agent-judged; the implicit one is silent」「Invisible housekeeping (no system-speak)」）。驗證:以可注入 goal-complete 訊號驗證反思被觸發且輸出不外送（靜默）;cargo build -p fleety-server 綠;實機端到端標手動驗證。

- [x] 2.3 把 post-turn housekeeping 移出連線迴圈成背景任務：在 conn.rs 把現有 `maybe_reflect`（skill 反思）與本變更的蒸餾/rollover 改成回應 emit 後 `tokio::spawn` 的背景 runner（不 inline await，使用者下一則訊息立即處理）；runner 用 economy tier（ProviderTiers cheap、FLEETY_CHEAP_MODEL_*）；single-flight per conversation（同對話在跑就略過第二個）；背景失敗只 log、不影響 live 對話——交付 "Housekeeping never blocks the user"（決策「Housekeeping runs in the background, off the user's interactive path」）。驗證:非阻塞測試（背景 housekeeping 還在跑時，後續訊息仍被處理）、single-flight 略過第二個、使用 cheap tier 的單元測試（以可注入 slow housekeeping/clock）;cargo build -p fleety-server 綠;實機標手動驗證。

## 3. 蒸餾路由與提示

- [x] 3.1 [P] 在 prompts/rules.md 加：蒸餾按類型分流規則（耐久知識/洞見→wiki、待辦→TODO、使用者事實→USER、裝置操作事實→device NOTES、純回顧→不寫，recall 已保存；wiki 存智慧非逐字摘要）＋ rollover 指引 ＋「隱式整理絕不以系統腔對使用者說話」規則——交付 "Takeaways are distilled into the right memory layer by kind"、"Invisible housekeeping" 的行為規範（決策「Distillation is type-routed by the agent, through existing tools」「Invisible housekeeping (no system-speak)」）。驗證:內容審查（規則涵蓋五類路由 + 隱式靜默）。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機端到端 rollover/蒸餾需手動驗證。
