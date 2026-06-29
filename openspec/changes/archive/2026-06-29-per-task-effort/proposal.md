## Why

模型請求目前完全不帶推理程度(reasoning effort / thinking budget):openai.rs 與 gemini.rs 的請求 body 沒有 reasoning_effort / thinking 等欄位,所以即使端點支援不同推理層級,fleety 永遠用端點預設。我們希望:簡單任務省算力、難任務多想。兩個面向——主 agent 應能依需求**動態自調**自己後續回合的推理程度;subagent 的推理程度則應由**生它的 agent**在 spawn 時決定(與現有 tier 選擇同一個 pattern,subagent 不自選)。

## What Changes

- 定義 `Effort`(low / medium / high)並讓模型呼叫可帶**可選 effort**;provider 依模型家族把它翻成對應欄位(OpenAI 相容 → `reasoning_effort`;Gemini → thinking 設定);模型若不支援 effort,則**省略不送**(安全 no-op)。
- **主 agent 動態自調**:提供一個工具讓 agent 設定自己後續回合的推理程度(session 層的 effort,持續到再次變更)。
- **subagent 由 parent 決定**:`SpawnRequest` 增加 `effort` 欄位,由發起 spawn 的 agent 指定;未指定則繼承預設。subagent 自身不選自己的 effort。
- 預設 effort 可由設定指定(`FLEETY_MODEL_EFFORT` / `FLEETY_CHEAP_MODEL_EFFORT`)。

## Non-Goals

(本變更會建立 design.md,Non-Goals 寫在 design 的 Goals/Non-Goals 一節。)

## Capabilities

### New Capabilities

- `per-task-effort`: 可選的推理程度貫穿模型呼叫——Effort 型別、provider 依家族翻成對應欄位(不支援則省略)、主 agent 以工具動態自調自己的 effort、subagent 的 effort 由 parent 在 spawn 時決定、預設可設定。

### Modified Capabilities

(none)

## Impact

- Affected specs: per-task-effort(新)
- Affected code:
  - Modified:
    - crates/agent-core/src/model.rs(Effort 型別;模型呼叫帶可選 effort)
    - crates/agent-core/src/openai.rs(把 effort 翻成 reasoning_effort,不支援則省略)
    - crates/agent-core/src/gemini.rs(把 effort 翻成 thinking 設定,不支援則省略)
    - crates/agent-core/src/agent.rs(turn 把 session effort 傳給 provider 呼叫)
    - crates/agent-core/src/subagent.rs(SpawnRequest 加 effort,由 parent 指定)
    - crates/fleety-server/src/providers.rs(預設 effort 與 effort-scheme 注入)
    - crates/fleety-tools/src/config.rs(effort 設定鍵)
    - docs/env.md(記錄 effort 設定)
  - New:
    - crates/fleety-server 內一個讓主 agent 自調 effort 的工具(set_effort)
