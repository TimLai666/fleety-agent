## Context

effort 機制的現況(見 `crates/fleety-server/src/effort.rs`、`crates/fleety-server/src/conn.rs`):

- `SessionEffort = Arc<Mutex<Option<Effort>>>` 是 per-connection 的 slot,`set_effort` 工具寫入、連線迴圈讀取。
- 連線迴圈在處理一則 top-level 使用者訊息的最開頭讀一次 slot,選出 `turn_provider`(以 `provider.with_effort(Some(e))` 取得 effort variant),然後把這個**已定型的 provider** 傳進 `drive_to_goal`。
- `drive_to_goal` 的簽章只收 `provider: &dyn ModelProvider`,不收 `session_effort`;它內部的 goal 連續迴圈逐次呼叫 `drive_turn`,整段沿用同一個 provider。

因此 `set_effort` 在請求中途被呼叫時,只更新了 slot,但當前請求(含所有 goal 連續 turn 與排隊後續訊息)不會重讀,高 effort 要到下一則 top-level 訊息才生效。同時 `prompts/` 沒有任何 effort 使用指引,工具描述也讓 agent 誤以為當前請求會受惠。

約束:沿用既有 `Effort` / `with_effort` / `effort_field` 機制與 `mid-turn-interruption` 的 triage 範式(純解析 + 便宜模型 + graceful 降級);不動 subagent 的 effort 模型。

## Goals / Non-Goals

**Goals:**

- 讓 agent 在請求中途的 `set_effort` 於**下一個 goal 連續 turn** 立即生效,不再拖到下一則使用者訊息。
- 在**第一次推理之前**依使用者訊息難度自動選 effort,讓難任務一開始就在對的檔位,不依賴 agent 自覺。
- 定義清楚的優先序,並讓 agent 能把控制權交還自動分類。
- 修正誤導的工具描述,補上 prompt 指引。

**Non-Goals:**

- 不在 `agent-core` 內層工具迴圈逐次推理重選 provider(重讀粒度只到 `drive_turn` 邊界)。
- 不改 subagent spawn-time effort 模型。
- 不改 provider 對 effort 的編碼(OpenAI/Gemini scheme)。

## Decisions

### 決策一:重讀粒度 = goal 連續 turn(drive_turn 邊界)

把 `session_effort: &SessionEffort`、基礎 `provider: &dyn ModelProvider`、以及本 turn 的 `turn_baseline: Option<Effort>` 一起傳進 `drive_to_goal`。在 goal 連續迴圈**每次 `drive_turn` 之前**,計算該連續 turn 的生效 effort 並以 `provider.with_effort(...)` 重選 provider(None 時直接用基礎 provider)。

- 替代 A(逐內層推理重讀):需侵入 `agent-core` 的內層工具迴圈,改動面大且與 provider-agnostic 邊界衝突;延後為未來強化。
- 替代 B(維持只讀一次):即現行 bug,否決。
- 生效邊界:mid-request `set_effort` 於下一個 `drive_turn` 生效。單一 `drive_turn` 內、set_effort 之後的剩餘內層推理仍用該 turn 起始的檔位;此缺口由決策二在首次推理前補上。

### 決策二:難度自動選 effort(turn 起始前的分類器)

在 `crates/fleety-server/src/effort.rs` 新增:

- 純函式 `parse_effort(model_text: &str) -> Option<Effort>`:把模型的一詞回答映射為 low/medium/high,無法判定回 `None`。
- `assess_effort(new_msg: &str, provider: &dyn ModelProvider) -> Option<Effort>`:仿 `triage`,以一次便宜模型呼叫請模型就訊息難度回一個詞;呼叫失敗或不可解析回 `None`(保守,不改變預設)。

呼叫點在 `conn.rs` 的 turn 起始(現行讀 `session_effort` 之處):當 `FLEETY_AUTO_EFFORT` 開啟且**沒有手動釘選**時執行,結果即本 top-level turn 的 `turn_baseline`。分類器優先使用 cheap tier provider(economy-model-tier);無 cheap 時退回主 provider。空白訊息略過分類。

- 替代(長度啟發式判難度):過於粗糙、易誤判;否決,改用模型分類。

### 決策三:優先序,且自動結果不寫入 slot

每個 top-level turn 與每個連續 turn 的生效 effort:

```
effective = manual_pin(session_effort slot).or(turn_baseline).or(None → provider 內建預設)
```

關鍵:**自動分類的結果不寫回 `session_effort` slot**,只以區域變數 `turn_baseline` 承載並傳入 `drive_to_goal`。理由:slot 的 `Some(_)` 語意是「agent 手動釘選、跨訊息持久」;若把自動結果寫入,會被誤認為手動釘選並在後續訊息持久污染。因此:

- agent 中途 `set_effort(high)` → slot=`Some(High)` → 之後連續 turn 壓過 baseline 用 high。
- agent `set_effort(auto)` → slot=`None` → 回到 `turn_baseline`(自動或無)。
- agent 不動 → 用 `turn_baseline`。

`set_effort` 工具的 `level` enum 增加 `auto`:呼叫時把 slot 設為 `None`(清除釘選);其餘 low/medium/high 維持寫入 `Some(_)`。工具描述改為明講「不影響當前這一步,從下一個連續 turn / 下一則訊息起生效並持久,直到再次變更;傳 auto 交還自動」。

### 決策四:設定 FLEETY_AUTO_EFFORT

在 `crates/fleety-tools/src/config.rs` 的型別化 registry 新增 `FLEETY_AUTO_EFFORT`(scope Shared,預設 on)。off → 跳過分類器、`turn_baseline` 恆為 `None`,行為回到純手動 `set_effort`。既有 `FLEETY_MODEL_EFFORT` / `FLEETY_CHEAP_MODEL_EFFORT` 作為 provider 內建預設,構成優先序最後一層,無需在此重複處理。

## Implementation Contract

**Behavior:**

- 使用者送出非空白訊息且 `FLEETY_AUTO_EFFORT` on 且無手動釘選 → runtime 以便宜模型判難度,第一次推理即用該 effort。
- agent 在請求中途 `set_effort(high|medium|low)` → 下一個 goal 連續 turn 起以該 effort 發模型呼叫,並跨後續訊息持久,直到再次變更。
- agent `set_effort(auto)` → 清除手動釘選,之後回到自動分類 / 預設。
- `FLEETY_AUTO_EFFORT` off → 不做分類,只有手動 `set_effort` 影響 effort。

**Interface / data shape:**

- `set_effort` 工具 `level` enum:`low | medium | high | auto`;`auto` 將 `session_effort` 設為 `None`。
- `parse_effort(text: &str) -> Option<Effort>`(pure);`assess_effort(new_msg: &str, provider: &dyn ModelProvider) -> Option<Effort>`。
- `drive_to_goal` 簽章新增 `session_effort: &SessionEffort` 與 `turn_baseline: Option<Effort>`;其 `provider` 參數語意改為「基礎 provider」,由函式內每連續 turn 自行套用 effort。
- 新 config key `FLEETY_AUTO_EFFORT`(on/off),出現在 `fleety config list`。

**Failure modes:**

- 分類呼叫失敗或回答不可解析 → `assess_effort` 回 `None`,沿用預設,不阻斷、不報錯。
- cheap provider 不可用 → 退回主 provider 做分類。
- auto off 或訊息空白 → 略過分類。

**Acceptance criteria:**

- 單元測試:`parse_effort` 對 low/medium/high 與雜訊/空字串(→ None)的映射表;`set_effort(auto)` 後 slot 為 `None`,low/medium/high 後為對應 `Some`。
- 行為測試:以記錄「每次 `drive_turn` 收到的 effort」的測試替身 provider 驗證 `drive_to_goal` 會在連續 turn 間 pick up slot 的新值(釘選壓過 baseline;auto 回 baseline)。
- `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` 全乾淨。

**Scope boundaries:**

- In:`crates/fleety-server/src/effort.rs`、`crates/fleety-server/src/conn.rs`(turn 起始 + `drive_to_goal`)、`crates/fleety-tools/src/config.rs`、`prompts/protocol.md`、`prompts/rules.md`、`docs/env.md`、`docs/tools.md`、`per-task-effort` spec。
- Out:agent-core 內層逐推理重讀、subagent effort、provider effort 編碼。

## Risks / Trade-offs

- **每 top-level turn 多一次便宜模型呼叫**(延遲/成本)。緩解:僅在 auto on 且無釘選時執行;用 cheap tier;空白訊息略過;`FLEETY_AUTO_EFFORT=off` 可全關。
- **分類器誤判** → 用錯檔位。緩解:不確定回 `None`(保守用預設);agent 可隨時手動壓過。
- **重讀粒度限制**:單一 `drive_turn` 內剩餘內層推理仍用該 turn 起始檔位;已由決策二在首次推理前補、並將逐推理重讀列為未來強化。
- **持久污染防護**:自動結果刻意不寫 slot,避免被當成手動釘選而在後續訊息持久。
