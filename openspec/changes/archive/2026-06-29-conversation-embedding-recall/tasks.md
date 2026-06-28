<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機真實 embedding 排序品質為環境相依（會下載 ~300MB 模型），需手動驗證。 -->

## 1. 共用 embedding 存取層

- [x] 1.1 在 crates/fleety-server/src/wiki_embed.rs 把模型存取抽成共用 helper（load-once fastembed 模型、`embed_texts`/query embed、DOC/QUERY 前綴、`FLEETY_WIKI_EMBED` gate、模型快取目錄），wiki 索引改呼叫它、行為不變——交付供兩個索引共用一個模型的基礎（決策「Reuse the wiki's embedding layer via a small shared accessor」）。驗證:wiki 既有語意搜尋測試全綠（重構不改行為）;cargo test -p fleety-server wiki 綠。

## 2. per-user 對話向量索引

- [x] 2.1 [P] 新增 crates/fleety-server/src/conversation_embed.rs 的**儲存/查詢層**（可注入預算向量、不載模型）：建立/開啟 per-user sqlite-vec db（`meta(model,dim)`、`chunks(key,conversation_id,seq,ts_secs,role,snippet)`、`vec0` `vec_chunks`），`upsert_vectors`、`knn(query_vec, limit) -> Vec<(meta,score)>`——交付 "Per-user conversation vector index" 的可測核心（決策「Separate the index storage/query layer from the model」「A per-user conversation index, one embedding per message」）。驗證:純測試（合成向量）插入後 KNN 依 cosine 取最近、meta(dim) round-trip、空索引無結果;cargo test -p fleety-server conversation_embed 綠。
- [x] 2.2 在 crates/fleety-server/src/conversation_embed.rs 接上 embedding 步驟（用 1.1 的共用存取層把訊息內容 embed 成向量再 upsert；query embed）＋ crates/fleety-server/src/storage.rs 加 `conversation_index_path(user)`——交付 "Per-user conversation vector index" 的完整建置（決策「A per-user conversation index, one embedding per message」）。驗證:`conversation_index_path` 路徑在 `fleet/users/<user>/conversations/.index/` 下的單元測試;cargo build -p fleety-server 綠;真實 embed 標手動驗證。

## 3. 語意搜尋升級 + 接線

- [x] 3.1 在 crates/fleety-server/src/conversation_recall.rs 讓 `ConversationSemanticSearch::call` embed query→KNN→映射成 `RecallHit`（score=cosine，依 score 再 ts 新→舊），embeddings 關閉/索引空/不可用時退回 `keyword_search`，Guest 回空——交付 "Semantic conversation search, with keyword fallback" 與 "Semantic recall is embedding-backed"（決策「Search: embed the query, KNN, map to RecallHit, keyword fallback」）。驗證:測試「FLEETY_WIKI_EMBED=0 或空索引→回 keyword 結果不報錯」「Guest→空」「KNN keys→RecallHit 帶 score 且排序正確」（合成向量）;cargo test -p fleety-server conversation 綠;真實排序品質標手動驗證。
- [x] 3.2 在 crates/fleety-server/src/conn.rs 回合結束後對 acting user 的索引做 off-turn 增量更新（fire-and-forget，失敗只 log 不影響回合）＋ 搜尋時索引缺/落後則 bounded backfill——交付 "Per-user conversation vector index" 的增量更新（決策「Incremental, off-turn indexing + lazy backfill」）。驗證:hook 立即返回、錯誤被吞不致回合失敗（以可注入/同步點驗證）;cargo build -p fleety-server 綠;實機長對話增量索引標手動驗證。

## 4. 文件

- [x] 4.1 [P] docs/env.md：更新 conversation recall 段，說明語意搜尋現為真實 embedding（沿用本機 EmbeddingGemma + `FLEETY_WIKI_EMBED` gate、per-user 索引、off-turn 增量、缺/關時退回關鍵字）——交付:文件與行為一致。驗證:內容審查。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心未受影響——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認仍無 fleety-*;記錄真實 embedding 排序品質需手動驗證（會下載模型）。
