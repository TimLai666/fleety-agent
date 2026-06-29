## Context

模型請求不帶任何推理程度欄位;openai.rs / gemini.rs 只送 model/messages/tools(+stream)。各家對「推理程度」的表示不同:OpenAI 相容用 `reasoning_effort`(low/medium/high,限 o 系列 / gpt-5 等);Gemini 用 thinking 設定;多數一般模型不接受這類欄位(送了會被拒)。子代理已能由 parent 在 spawn 時選 tier(SpawnRequest.tier / "model" 參數);effort 的歸屬與此一致:**subagent 的 effort 由 parent 決定,主 agent 的 effort 由它自己動態調**。約束:agent-core 不依賴 fleety crate;forbid unsafe;never-crash;env 測試單執行緒。

## Goals / Non-Goals

**Goals:**

- 模型呼叫可帶可選 effort;provider 依模型家族翻成對應欄位,不支援則省略(不讓「帶 effort」本身造成失敗)。
- 主 agent 能在對話過程中動態改變自己後續回合的推理程度。
- subagent 的推理程度由發起 spawn 的 agent 指定(預設繼承),subagent 不自選。
- 預設 effort 可由設定指定。

**Non-Goals:**

- 不自動「依任務難度猜 effort」——effort 由 agent 顯式決定(主 agent 自調、subagent 由 parent 給),不做自動分類器。
- 不改 wire 協定(client↔server);不引入新依賴。
- 不保證所有端點都支援 effort——不支援就省略。

## Decisions

### Effort 型別與可選地貫穿模型呼叫

定義 `enum Effort { Low, Medium, High }`(可序列化、可由字串解析)。模型呼叫帶 `Option<Effort>`:在 `ModelProvider::complete` / `complete_streaming` 增加一個 `effort: Option<Effort>` 參數(附加式簽章變更),所有 provider 與呼叫端一併更新;`None` 表示不指定(沿用端點預設)。理由:effort 是 per-call 的輸入,直接當參數最直接;附加參數雖動到 trait 簽章但機械且範圍可控。

### provider 依家族翻譯,不支援則省略

provider 持有一個 `effort_scheme`(`None` / `OpenAiReasoning` / `GeminiThinking`),由模型名稱啟發式或設定決定。送請求時:scheme 為 `OpenAiReasoning` → 加 `reasoning_effort: "<low|medium|high>"`;`GeminiThinking` → 加對應 thinking 欄位;`None` → 完全不加 effort 欄位。翻譯做成純函式以利測試。理由:把「這個模型能不能吃 effort、欄位長怎樣」隔離成可測映射,避免對不支援的模型盲送導致 4xx。

### 主 agent 動態自調自己的 effort(session 狀態 + 工具)

新增一個工具(set_effort),讓主 agent 設定自己後續回合的推理程度;該值存在 session/對話層的狀態(類似 GoalState 的存法),turn loop 每回合把目前 session effort 當作 `effort` 傳給 provider 呼叫;直到 agent 再次變更。理由:符合「agent 依需求自己變」——它在判斷接下來要深思時自己拉高,瑣事時調低。

### subagent 的 effort 由 parent 在 spawn 時決定

`SpawnRequest` 增加 `effort: Option<Effort>`,由發起 spawn 的工具參數帶入(例如 spawn 工具接受 `"effort": "high"`);未指定則繼承一個預設(設定值或父的 session effort)。subagent 執行時用這個固定 effort,**不提供讓 subagent 自調自己 effort 的途徑**。理由:符合「subagent 推理程度由生它的 agent 決定」。

### 預設 effort 可設定

`FLEETY_MODEL_EFFORT` / `FLEETY_CHEAP_MODEL_EFFORT` 提供各 tier 的預設 effort(未設定 → None=不指定)。登記到 typed config registry。理由:作業者可定錨,agent/parent 可再覆寫。

## Implementation Contract

**行為(Behavior):**

- 帶 effort 呼叫支援 effort 的模型 → 請求含對應欄位(reasoning_effort / thinking)。
- 帶 effort 呼叫不支援的模型 → 請求**不含** effort 欄位(不致失敗)。
- 主 agent 呼叫 set_effort(high)後,其後續回合的模型呼叫帶 high,直到再次變更。
- parent spawn subagent 帶 effort=low → 該 subagent 所有回合用 low;未帶 → 用預設。
- 未設定任何 effort 且 agent 未調整 → 行為等同現況(不送 effort 欄位)。

**介面 / 資料形狀:**

- model.rs:`enum Effort { Low, Medium, High }`(+ `from_str` / `as_str`);`ModelProvider::complete(&self, &[Message], &[ToolSpec], effort: Option<Effort>)` 與 `complete_streaming(..., effort: Option<Effort>)`(附加參數)。
- 純函式:`reasoning_effort_field(scheme, effort) -> Option<(&str, Value)>`(或等義),測試其對三種 scheme 的輸出。
- subagent.rs:`SpawnRequest.effort: Option<Effort>`;spawn 工具參數 `"effort"`。
- session 層 effort 狀態(存放處與 GoalState 同層)+ set_effort 工具(在 fleety-server 註冊)。
- providers.rs:依模型名稱/設定決定 effort_scheme 與預設 effort 並注入 provider。
- config registry:`FLEETY_MODEL_EFFORT`、`FLEETY_CHEAP_MODEL_EFFORT`。

**失敗模式:**

- effort 字串無法解析 → 視為 None(不指定),不 panic。
- scheme=None 仍帶 effort → 省略,不送。

**驗收標準(Acceptance):**

- 單元測試:effort 字串解析(low/medium/high/未知→None);`reasoning_effort_field` 對 OpenAiReasoning/GeminiThinking/None 三種輸出(None→不產欄位)。
- 單元測試:openai/gemini 在 scheme=None 時請求 body 不含 effort 欄位;在對應 scheme 時含正確欄位。
- 單元測試:set_effort 改 session effort 後,下一次 turn 傳入的 effort 改變;SpawnRequest.effort 由 parent 帶入、subagent 沿用。
- 既有 provider/subagent 測試全綠(effort=None 行為相容)。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒可跑。
- HTTP 實際往返(端點是否真的依 effort 改變推理)為手動驗證。

**範圍邊界:**

- In scope:Effort 型別、provider 翻譯+省略、主 agent set_effort 工具與 session 狀態、subagent spawn 的 effort 參數、預設設定、文件。
- Out of scope:自動依難度猜 effort、subagent 自調、wire 協定變更、保證端點行為。

## Risks / Trade-offs

- [trait 簽章加參數,動到所有 provider 與呼叫端] → 機械式更新;`None` 維持相容;一次到位。
- [對不支援 effort 的模型誤送 → 4xx] → effort_scheme=None 時省略;scheme 由名稱/設定保守判定。
- [各家 effort 語意不完全對齊(low/med/high vs token budget)] → 先用三段式對應;Gemini 的 thinking 以粗略對應,細節列 Open Question。

## Migration Plan

- 附加式:effort 預設 None → 不送欄位 → 行為等同現況。要啟用就設定預設或讓 agent 自調 / parent 指定。
- 無資料遷移、不改 wire。

## Open Questions

- Gemini thinking 設定與 low/med/high 的精確對應(token budget 對應值)留待 apply 時定。
- 是否讓「未指定時」自動採 parent session effort 作為 subagent 預設,或一律用設定預設:初版用設定預設,parent 可顯式覆寫。
