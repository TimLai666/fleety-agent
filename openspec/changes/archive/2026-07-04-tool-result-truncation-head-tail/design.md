## Context

工具結果回饋模型前經 `compress_tool_result`(agent-core/src/compress.rs):先 SmartCrusher 結構壓縮(長陣列→頭 20 + 尾 5;長字串→取前 max_string 字元;深度上限),再 budget 總量截斷(預設 8000 字元)。完整結果永遠存 event log,`fetch_tool_result`(fleety-server/src/tools.rs)以字元窗口分頁取回。

兩個確認缺陷:
- A:長字串截斷只保留開頭(`chars().take(max_string)`),尾端全丟。run_command 的 stdout/stderr 是單一字串,尾端結論看不到。長陣列已保留頭尾,字串不一致。
- B:fetch 一頁的 content 上限是 budget(8000),但 fetch 結果回到 run_turn 又經一次壓縮,SmartCrusher 的 max_string(4000)把 content 再砍半並掛一個指向 fetch 自身的 marker。budget 與 max_string 不一致造成遞迴截斷。

## Goals / Non-Goals

**Goals:**

- 長字串截斷保留頭與尾,對齊長陣列的頭尾精神,讓單一字串輸出的尾端結論可見。
- fetch 一頁 content 抵達模型時原樣、不被二次結構截、不冒出指向自身的 marker。
- 兩者各附回歸測試;既有截斷/可逆/marker 行為不回歸。

**Non-Goals:**

- 不改 budget(8000)與 max_string(4000)的數值本身。
- 不改 event log 可逆機制、不改 fetch 分頁介面(offset/limit/next_offset)的形狀。
- 不動 CodeCompressor / CacheAligner。
- 不改長陣列的頭 20 + 尾 5 邏輯。

## Decisions

### 長字串結構壓縮保留頭與尾

SmartCrusher 對超過 max_string 的字串,改為保留開頭一段與結尾一段,中間放省略標記(頭佔約 3/4、尾佔約 1/4,總保留字元數維持約 max_string,加一個「…(+N chars omitted)」標記)。以 `chars()` 為單位切割(UTF-8 安全),`crush_tracked` 仍標記 truncated=true(內容確有遺失,fetch id 照掛)。
替代方案:(a) 只留頭(現況)——尾端結論丟失;(b) 頭尾各半——run_command 開頭的指令回顯 context 也常重要,頭多一點較穩。故取頭 3/4、尾 1/4。

### fetch 一頁上限對齊 SmartCrusher 長字串門檻

`fetch_tool_result` 一頁 content 的上限(limit 的 clamp 上界)從 budget 改為對齊 SmartCrusher 的長字串門檻(max_string)。這樣一頁 content 不超過該門檻,回到 `compress_tool_result` 時 SmartCrusher 不會再截它(`n > max_string` 在等於時為 false),整段 JSON 也遠低於 budget,於是 truncated=false、不掛任何 marker,一頁原樣抵達模型。門檻值由 agent-core 單一來源提供,fetch 引用之,避免兩處各寫一個數字。
替代方案:(a) 另建「只做 budget、跳過結構壓縮」的回饋路徑並識別 fetch 工具名——需在 run_turn 硬編工具名 + 處理 JSON 包裝仍可能超 budget 的邊界,較複雜且耦合;(b) 把 max_string 提高到等於 budget——會改門檻數值、改變長字串(含非 fetch 工具)的截斷點,超出本修範圍。故選對齊門檻:最小改動、零新機制、不碰 run_turn。代價是每頁上限較小(對齊到字串門檻而非 budget),分頁次數略增,但每頁保證完整。

## Implementation Contract

**A — 長字串頭尾(behavior):** SmartCrusher 結構壓縮一個長度 n 字元、且 n 大於 max_string 的字串時,輸出 = 開頭 head_len 個字元 + 「…(+omitted chars omitted)」標記 + 結尾 tail_len 個字元,其中 head_len ≈ max_string 的 3/4、tail_len = max_string − head_len、omitted = n − head_len − tail_len。切割以字元計。長度 ≤ max_string 的字串維持原樣。此路徑仍將 truncated 旗標設為 true,使既有的 fetch-id marker 行為不變。

**B — fetch 一頁對齊門檻(behavior + data shape):** `fetch_tool_result` 的 limit 上界 clamp 到 agent-core 提供的長字串門檻(而非 budget);未指定 limit 時亦預設為該門檻。回傳 JSON 欄位(id/found/total_chars/offset/returned_chars/next_offset/content)形狀不變;total_chars 與 next_offset 的分頁語意不變。門檻由 agent-core 暴露(SmartCrusher 的長字串門檻,單一來源),fleety-server 的 fetch 引用之。

**Failure modes:** 空字串 / 短字串不進頭尾路徑(原樣)。fetch offset 超出總長時 content 空、next_offset null(既有行為不變)。fetch 一頁對齊門檻後,經 compress_tool_result 不再產生 truncated,故不掛 marker。

**Acceptance criteria:**
- `long_string_keeps_head_and_tail`:一個遠超 max_string 的字串經 SmartCrusher 後,開頭與結尾的原始片段都在、中間有省略標記、總字元數約為 max_string 量級。
- `short_string_unchanged`:長度 ≤ max_string 的字串原樣返回。
- `fetch_page_capped_to_threshold`:對 fetch 傳超過門檻的 limit,回傳 content 長度被 clamp 到門檻。
- `fetch_page_survives_compression`:一頁 fetch 結果(content 為門檻大小)經 compress_tool_result 後,content 原樣、不含 `fetch_tool_result` marker。
- 既有測試(long_string_is_truncated、within_budget_but_crushed_result_names_fetch_id、fetch 分頁測試)更新為新行為且全綠。

**Scope 邊界:** in scope —— SmartCrusher 長字串頭尾(compress.rs)、agent-core 暴露門檻常數、fetch limit 對齊門檻(tools.rs)、上述測試。out of scope —— budget/門檻數值、event log、fetch 分頁介面形狀、CodeCompressor/CacheAligner、長陣列邏輯、run_turn 壓縮呼叫點。

## Risks / Trade-offs

- [頭尾切割切到多位元組字元中間] → 以 `chars()` 為單位切,不會切壞 UTF-8。
- [fetch 每頁上限變小(門檻 < budget)] → 分頁次數略增,但每頁保證完整、不再遞迴截斷;正確性優先。此取捨在 design 明載。
- [fetch 與門檻兩處各寫數字易漂移] → 門檻由 agent-core 單一來源暴露,fetch 引用,不重複字面量。
- [長字串頭尾標記讓 `within_budget_but_crushed` 類既有測試斷言變動] → 一併更新既有測試為頭尾預期。

## Migration Plan

純行為修正,無資料遷移、無介面形狀變更。部署後長字串輸出即含頭尾、fetch 一頁即完整。Rollback:還原 compress.rs 與 tools.rs 兩處即可。
