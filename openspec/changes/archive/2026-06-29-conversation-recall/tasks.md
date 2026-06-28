<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。真實 embedding KNN（需模型）為環境相依，需手動驗證。 -->

## 1. 對話事件時間戳（storage）

- [x] 1.1 在 crates/fleety-server/src/storage.rs 讓對話事件記錄改成 `{seq, ts_secs, message}`：append 寫入寫入當下 ts_secs、讀取以 serde default 0 相容舊 `{seq, message}` 行；resume/replay 行為不變；並提供一個逐事件 iterator（給關鍵字掃描與 backfill 用）——交付 "Conversation events carry a timestamp"（決策「Add `ts_secs` to conversation events, additively」）。驗證:單元測試 append→讀回有 ts_secs、舊行讀回 ts_secs==0、load_after 仍依序;cargo test -p fleety-server 綠。

## 2. 召回工具與索引

- [x] 2.1 新增 crates/fleety-server/src/conversation_recall.rs：`RecallHit { conversation_id, seq, ts_secs, role, snippet, score }` 與工具 `conversation_search`（關鍵字掃 JSONL、newest-first）、`conversation_semantic_search`（複用 wiki 的 embedding+sqlite-vec 層對 per-device 對話索引做 KNN、回 score+ts；模型不可用時退回關鍵字並附註）、`conversation_list`（列該裝置對話 + last-activity ts）；把 wiki_embed 的 embed-query 與 sqlite-vec 呼叫抽出共用（wiki 行為不變）；recall 索引路徑 fleet/devices/<device>/.recall/recall.db；全部 per-device scope——交付 "The agent can search its past conversations"、"Recall is best-effort and degrades without embeddings" 的查詢面（決策「Recall is keyword + semantic, reusing the wiki embedding/sqlite-vec layer」「Per-device scope and result shape」）。驗證:關鍵字召回（不需模型）回正確 hits 與 seq+ts、newest-first;RecallHit/conversation_list 形狀測試;embeddings 關閉時 semantic 退回關鍵字且附註不報錯;共用 embedding 層以 temp index smoke test;真實 KNN 標手動驗證;cargo test -p fleety-server 綠。
- [x] 2.2 在 crates/fleety-server/src/conn.rs 於訊息 append 後背景索引該訊息（spawn、best-effort、受 FLEETY_WIKI_EMBED gate、永不阻擋/失敗 turn）；對預先存在或有缺口的對話於首次 semantic 搜尋時 lazy backfill（其間以關鍵字回應）——交付 "Recall is best-effort and degrades without embeddings" 的索引面（決策「Incremental best-effort indexing with lazy backfill」）。驗證:cargo build -p fleety-server 綠;「索引失敗不影響 turn」與「缺索引→排 backfill + 關鍵字回應」的邏輯測試/審查;真實索引標手動驗證。

## 3. 文件

- [x] 3.1 [P] docs/env.md：說明對話召回複用 FLEETY_WIKI_EMBED 與模型目錄、recall 索引位置、per-device scope、無模型時退回關鍵字——交付:文件與行為一致。驗證:內容審查。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄真實 embedding 召回需手動驗證。
