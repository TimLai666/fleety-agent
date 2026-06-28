## Context

Fleety 已有 authored-skill 自寫工具（`skill_write_file`，skills.rs 註解明寫 Hermes-style、agent 自主編輯）、`memory_write`、`wiki_write`，但沒有任何觸發去用它們做「學習迴圈」。`use_skill` 只回 SKILL.md 內容、不回目錄，故 skill 內 `scripts/` 的自寫工具沒有乾淨的執行路徑（只能 `list_skills` 拿絕對目錄再 `run_command` 拼）。

剛完成的 goal-completion 在 conn 的 `drive_to_goal`/`drive_turn` 有「終止回合」與 `TurnReply`/`TurnOutcome.steps`；反思 nudge 正好掛在 goal 終止之後。使用者已拍板：(1) 觸發＝Prompt + runtime nudge（硬，比照 goal）；(2) skill 腳本執行＝路徑 + run_command 約定（`use_skill` 回 path、不新增執行工具）。

## Goals / Non-Goals

**Goals:**
- 一則使用者訊息在 goal 終止後，若工具步數達門檻，runtime 跑「一次」有界反思回合，讓 agent 把可重用流程存成/更新 authored skill、把 durable 事實寫進 memory/wiki。
- 未達門檻或關閉時完全不跑（不花 token）；反思回合不可再觸發反思（有界、不遞迴）。
- `use_skill` 回傳加上 skill 絕對目錄 `path`，讓 SKILL.md 以 `run_command` 跑 `scripts/` 自寫工具的約定可行。
- prompt 教學習迴圈與記憶落點判準。

**Non-Goals:**
- 不新增專屬的 skill-script 執行工具（沿用 run_command + path 約定）。
- 不做背景非同步 self-improvement／定時 nudge（只在使用者訊息終止後同步觸發一次）。
- 不改 goal 迴圈或 voice 機制本身；反思回合本身不設 goal、不產 voice speech。
- 不動 agent-core（變更落在 fleety-server/conn、skills、prompts、docs）。

## Decisions

### 反思 nudge：goal 終止後依步數門檻跑一次有界反思回合

conn 的 UserMessage 流程：`drive_to_goal` 回來後拿到該訊息累積工具步數 `steps`；若 `min_steps > 0 且 steps >= min_steps`，呼叫 `maybe_reflect` 跑「一次」反思回合——用既有頂層 registry（含 `skill_write_file`/`memory_write`/`wiki_write`/`run_command`）以一段反思 seed 當 user 訊息跑 `drive_turn`。反思回合不再回頭檢查步數、不再呼叫自己，故有界、不遞迴。整個動作在既有 per-message turn_lock 內，不與背景 wake turn 交錯。門檻與開關用 env（見下）。

**反思回合輸出：** 採 `emit_terminal=true`，讓使用者看到一行結果（「已存 skill X／已記 …／無可存」）——因為它會做 mutate（寫 skill/memory），透明回報比靜默好；seed 要求 agent 簡短（一行；無可存就一句帶過）。

**替代方案：** (a) 靜默丟棄反思輸出——否決，mutate 不該無聲。(b) 背景非同步反思——否決，增加並發與排序複雜度，且同步一次已足夠（Hermes 也以任務後為主）。

### 複雜度啟發式：累加各回合的工具步數

以 `drive_to_goal` 內每個 `drive_turn` 的 `TurnOutcome.steps`（該回合的 provider 步數，含工具輪）累加為複雜度代理量。為此 `TurnReply` 加 `steps: usize`、`drive_turn` 從 `outcome.steps` 帶出，`drive_to_goal` 改回傳 `usize`（總步數）。代理量定義：步數越多＝工具輪越多＝越可能有可沉澱的流程，門檻比照 Hermes「5+」。

**替代方案：** 數 event log 的 ToolCall 次數——更精準但要多走一圈 events；步數累加已足夠當門檻代理量，從簡。

### 記憶落點判準（skill / memory / wiki / 不記）

寫進 prompt 教 agent：可重用**流程** → authored skill（必要時 `scripts/` 放自寫工具腳本、SKILL.md 提到並以 run_command 約定執行）；durable 的**使用者/專案事實** → memory（ME/USER）或 wiki（依既有 memory.md 規則）；只對當前對話有意義 → 不記。不重複記 code/git 已可得知的東西；矛盾不靜默覆寫（沿用 wiki 規則）。

### use_skill 回傳加上 skill 絕對目錄 path

`use_skill` 的回傳 JSON 除 `name`/`source`/`content` 外，再加 `path`（= SKILL.md 所在目錄的絕對路徑，與 `list_skills` 的 `path` 一致）。純新增欄位，既有呼叫端不受影響（向後相容）。這讓「用 `run_command` 跑 `<path>/scripts/xxx`」的約定可直接成立。

### skill 自寫工具以 run_command 約定執行

不新增執行工具。約定（寫進 docs/prompts）：skill 把自寫工具放 `scripts/`，在 SKILL.md 寫明如何用 `run_command` 執行（取 `use_skill` 回的 `path` 組絕對路徑；跨裝置時自行包 `device_exec`）。執行沿用 run_command 既有的 audit/rollback 與 policy gate。

**替代方案：** 新增 `skill_run_script` 一級工具——使用者已否決（這次走輕量約定；若日後要稽核/scope 專屬化再加）。

### 與 goal / voice 的互動與邊界

反思回合是 goal 終止「之後」的獨立 `drive_turn`，本身不設 goal（不進 drive_to_goal、GoalState 不重設）、voice=false（不產 speech）。它在 UserMessage arm、同一 turn_lock 內、`drive_to_goal` 之後執行，不影響 goal 迴圈與 emit_terminal。recovery／subagent wake 路徑不觸發反思。

## Implementation Contract

**Behavior:** 使用者一則訊息做完（goal 終止）後，若該訊息工具步數達 `FLEETY_SKILL_REFLECT_MIN_STEPS`（預設 5；0＝關閉），runtime 自動再跑一次反思回合：agent 視情況存/更新 authored skill（可含 `scripts/` 自寫工具）、寫 memory/wiki，並回一行結果。未達門檻或關閉：行為與現況完全相同（無額外回合、不花 token）。skill 內自寫工具可由 agent 取 `use_skill` 回的 `path` 後用 `run_command` 執行。

**Interfaces / data shapes:**
- env `FLEETY_SKILL_REFLECT_MIN_STEPS`：usize，預設 5；0 表關閉；解析失敗用預設。
- `conn::TurnReply` 加 `steps: usize`；`conn::drive_turn` 從 `TurnOutcome.steps` 填入。
- `conn::drive_to_goal` 回傳改為 `Result<usize>`（該訊息各回合步數總和）。
- `conn::maybe_reflect(out, storage, provider, tools, policy, device_id, conversation, gate, steps, min_steps)`：`min_steps>0 && steps>=min_steps` 時跑一次反思 `drive_turn`（emit_terminal=true、voice=false、單次、不遞迴）；否則 no-op。
- `skills.rs` 的 `use_skill` 回傳 JSON 新增 `path`（skill 目錄絕對路徑）。
- 反思 seed：一段固定 user 訊息，指示 agent 依落點判準存 skill / 記 memory-or-wiki，無可存則簡短帶過。

**Failure modes:** 反思回合若失敗，沿用 drive_turn 永不 crash（回錯誤訊息）；不應讓主回合的結果遺失（反思在主回合 emit 之後）。門檻 env 解析失敗→預設 5。use_skill 找不到 skill→維持既有錯誤。run_command 跑不存在的 script→既有 run_command 錯誤回饋，不 crash。

**Acceptance criteria:**
- conn 單元測試：一則訊息步數達門檻→反思回合確實多跑一次（以 out 接收端／provider 腳本消耗斷言）；步數未達或 `min_steps=0`→不跑反思。
- conn 單元測試：反思只跑一次、不遞迴（提供的腳本剛好夠一次反思，若多跑會耗盡 MockProvider 而失敗）。
- skills 單元測試：`use_skill` 回傳含 `path` 且指向該 skill 目錄；既有 `use_skill` 回傳欄位不變（向後相容）。
- 內容審查：prompts 有學習迴圈與落點判準；docs/env 有門檻 env；docs/tools 註明 use_skill 回 path + scripts 以 run_command 執行的約定。
- cargo fmt + clippy --workspace -D warnings + test --workspace 全綠；agent-core 不受影響（cargo tree 無 fleety-*）。

**Scope boundaries:**
- In：conn 的步數累計與門檻反思回合、`TurnReply.steps`、`drive_to_goal` 回傳步數、`maybe_reflect`、env、`use_skill` 加 path、prompts（學習迴圈＋落點判準＋scripts 執行約定）、docs、測試。
- Out：專屬 skill-script 執行工具、背景非同步／定時反思、goal/voice 機制改動、agent-core 改動、write_approval staging。

## Risks / Trade-offs

- [反思回合多花一次 LLM 呼叫] → 只在步數達門檻才跑、且每訊息至多一次；未達門檻零成本；門檻可調、可關（=0）。
- [步數作為複雜度代理不精準] → 接受為門檻代理量；偏好寧可少觸發（門檻預設 5）；日後可改數 ToolCall。
- [agent 反思時亂存一堆瑣碎 skill/記憶] → prompt 落點判準明確要求「可重用／durable 才存、無則不存」、不重複 code/git 已知；authored skill 為 mutate（audit+rollback）可回溯。
- [run_command 約定執行不夠安全/可攜] → 沿用 run_command 既有 policy/audit；跨裝置要自行 device_exec；若日後需專屬稽核再加一級工具。
- [反思回合干擾 goal/voice] → 反思在 goal 終止之後、不設 goal、voice=false、同 turn_lock 內，邊界明確。
