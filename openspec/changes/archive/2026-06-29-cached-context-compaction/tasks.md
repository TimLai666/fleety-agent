<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機長對話的 LLM 增量摘要品質為環境相依，需手動驗證。 -->

## 1. agent-core：快取型別 + 增量壓縮

- [x] 1.1 [P] 在 crates/agent-core 新增 `CompactionCache { summary, summarized_up_to_seq, recent_keep, threshold }` 與純函式 `is_cache_usable(cache, history_max_seq, config) -> bool`（watermark 在歷史內且 config 相符）＋「依 watermark+split 取新中段」的純切片邏輯——交付 "The cache is a safe, derived optimization" 的判定核心（決策「A compaction cache = summary text + a sequence watermark」「Invalidation is conservative and always safe」「Watermark semantics tie to conversation `seq`」）。驗證:純函式單元測試（watermark 在內/超前、config 相符/不符 → usable 與否；新中段切片正確）;cargo test -p agent-core 全綠。
- [x] 1.2 在 crates/agent-core/src/agent.rs 讓 `compact_if_needed` 吃 `&mut Option<CompactionCache>`（或回傳更新後的 cache）：超過門檻且 cache 可用→只摘 watermark 後的新中段並 fold 進既有摘要、推進 watermark；無/失效 cache→整段摘要並 seed；**不做任何 I/O**（cache in→cache out）——交付 "Context compaction reuses a cached rolling summary"、"The cache is a safe, derived optimization" 的核心（決策「`agent-core` compaction takes a cache in and returns it out (host-free)」「Invalidation is conservative and always safe」）。驗證:以 scripted provider 測試「首次壓縮 seed cache+watermark」「第二回合只摘 delta、不重摘整段、推進 watermark」「config 改變→失效重算」;cargo test -p agent-core 綠;真實增量摘要品質標手動驗證。

## 2. 持久化與接線（fleety-server）

- [x] 2.1 [P] 在 crates/fleety-server/src/storage.rs 加 per-conversation 壓縮快取持久化：`load_compaction(user, conv) -> Option<CompactionCache>` / `save_compaction(user, conv, &cache)`（存 user-primary 對話旁，如 `<id>.compaction.json`；壞/缺→None）——交付 "The cache is a safe, derived optimization" 的持久化面（決策「The server persists the cache per conversation; loads before, saves after」）。驗證:save→load round-trip、缺→None、壞 JSON→None 的單元測試;cargo test -p fleety-server 綠。
- [x] 2.2 在 crates/fleety-server/src/conn.rs 於組 turn messages 前載入該對話的壓縮快取、傳入 run 迴圈、回合後存回更新的快取；save 失敗只 log 不影響回合——交付 "Context compaction reuses a cached rolling summary" 的接線（決策「The server persists the cache per conversation; loads before, saves after」）。驗證:以可注入 storage/provider 驗證「回合前載入、回合後存回」;cargo build -p fleety-server 綠;實機長對話標手動驗證。

## 3. 文件

- [x] 3.1 [P] docs/env.md：說明 context compaction 現為增量＋快取（摘要落 `<id>.compaction.json`、依 watermark 只摘新段、config 改變或編輯則重算；event stream 仍為真相）——交付:文件與行為一致。驗證:內容審查。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機增量摘要品質需手動驗證。
