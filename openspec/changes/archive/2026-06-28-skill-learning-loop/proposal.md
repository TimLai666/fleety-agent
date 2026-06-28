## Why

Fleety 已有 agent 自寫 skill 的工具（`skill_write_file`→authored tier）、寫記憶（`memory_write`）與 wiki（`wiki_write`），但**沒有任何東西去觸發它們**：agent 做完一件多步、被使用者糾正、或摸索出新流程的工作後，不會自動把可重用的流程沉澱成 skill、也不會把值得記的事實留進 memory/wiki，下次得從頭再學。Hermes Agent 用「複雜任務後自動建 skill + 背景 self-improvement」把這個學習迴圈做出來；我們要在 Fleety 補上同樣的迴圈，且比照 goal-completion 用「明確、可靠」的 runtime 機制（不只靠模型自覺）。另外，skill 目前能放 script 但**沒有乾淨的執行路徑**：`use_skill` 只回 SKILL.md 內容、不回目錄，agent 要跑自寫工具得自己拼絕對路徑。

## What Changes

- **反思 nudge（server 端，硬觸發）**：一則使用者訊息在 goal 終止回合結束後，若該訊息累積的工具步數達門檻（`FLEETY_SKILL_REFLECT_MIN_STEPS`，比照 Hermes「5+ tool calls」），runtime 注入**一次**反思回合：請 agent 判斷有無可重用流程要存成/更新 authored skill、有無 durable 事實要寫進 memory 或 wiki。反思回合只跑一次、不可再觸發反思（不遞迴、有界）。未達門檻或關閉時完全不跑（不花 token）。
- **記憶落點判準（prompt）**：教 agent —— 可重用流程 → authored skill（必要時在 `scripts/` 放自寫工具腳本、於 SKILL.md 提到）；durable 的使用者/專案事實 → memory（ME/USER）或 wiki；只對當前對話有意義 → 不記。比照既有記憶規則，不重複記 code/git 已可得知的東西。
- **skill 自寫工具可執行（路徑 + run_command 約定）**：`use_skill` 回傳加上 skill 的絕對目錄 `path`（與 `list_skills` 一致），SKILL.md 以約定「用 `run_command` 跑 `scripts/xxx`（跨裝置自行包 `device_exec`）」執行自寫工具。不新增專屬執行工具。
- **文件**：prompts 教此學習迴圈與落點判準；docs 補反思門檻 env 與 use_skill/scripts 約定。

## Non-Goals

（細節取捨見 design.md 的 Goals/Non-Goals。）

## Capabilities

### New Capabilities

- `skill-learning-loop`: 做完夠複雜的工作後，runtime 觸發一次有界的反思回合，讓 agent 把可重用流程沉澱成/更新 authored skill（可含於 `scripts/` 的自寫工具腳本、在 SKILL.md 提到並以 run_command 約定執行）並把 durable 事實寫進 memory/wiki；由複雜度門檻 env 控制、未達門檻不觸發。

### Modified Capabilities

- `skills-management`: `use_skill` 回傳除了內容外再加上 skill 的絕對目錄 `path`，使「以 run_command 執行 skill 內 `scripts/` 自寫工具」的約定可行（向後相容，純新增欄位）。

## Impact

- 受影響 specs：新增 skill-learning-loop；修改 skills-management。
- 受影響程式：
  - 修改：crates/fleety-server/src/conn.rs（UserMessage 流程在 drive_to_goal 之後依步數門檻跑一次反思回合；步數累計）、crates/fleety-server/src/skills.rs（use_skill 回傳加 path）、prompts/protocol.md、prompts/rules.md、docs/env.md、docs/tools.md
  - 新增：無（沿用既有 skill_write_file/memory_write/wiki_write/run_command 工具）
  - 移除：無
- 關鍵驗收：達門檻才觸發、未達不觸發且不花 token、反思回合有界不遞迴；use_skill 回傳含 path 且向後相容；agent-core 不受影響仍 host-free；workspace fmt + clippy -D + test 全綠。
