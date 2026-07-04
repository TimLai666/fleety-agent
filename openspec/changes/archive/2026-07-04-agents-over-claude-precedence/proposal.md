## Why

conversation-scoped-skills 與 instruction-file-injection 剛落地,同層生態的優先目前是 `.claude` > `.agents`(Claude 專屬覆蓋通用)。使用者定調的通用規則相反:**`.agents`(通用標準)覆蓋 `.claude`(Claude 專屬)**。這是一條橫跨 skills、指令檔、以及未來 plugin/Codex 相容層的通用 precedence 規則,方向必須先翻正——否則後續相容層都會建在反的基礎上。

## What Changes

- 統一同層 precedence 為 **`.agents` > `.claude`**(所有 scope、skills 與指令檔一致)。scope 分層(專案 > 使用者 > 全域)與深度規則(深層 > 淺層)不變。
- **skills**:`skill_sources` 每一層的來源對從 `[.claude/skills, .agents/skills]` 改為 `[.agents/skills, .claude/skills]`。因為 `collect_scoped` 反序掃描(最後掃者最高),把 `.agents` 放在對的前面會讓它最後掃、最高,故同名 skill 由 `.agents` 覆寫 `.claude`。
- **指令檔**:`collect_instruction_paths` 的每個專案層從 `[AGENTS.md, CLAUDE.md]` 改為 `[CLAUDE.md, AGENTS.md]`。指令檔是軟疊加、淺→深、後者更 specific,把 AGENTS.md 排到該層更後的位置使其更 specific(較高)。user 全域目前已是 CLAUDE.md 先、AGENTS.md 後(AGENTS 已較高),不動。
- 更新兩個純函式的回歸測試斷言(`skill_sources_layers_and_dedupes` 的首項變 `.agents/skills`;`collect_instruction_paths_layers_and_dedupes` 每層首項變 `CLAUDE.md`)。

## Non-Goals (optional)

- 不改 scope 分層(專案 > 使用者 > 全域)與深度規則。
- 不改全域四層順序(installed > authored > builtin > synced)。
- 不碰 plugin / hooks / Codex 相容(相容層藍圖的 ②③④,後續 change)。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `skills-management`: 同層規則從 `.claude/skills` > `.agents/skills` 翻為 `.agents/skills` > `.claude/skills`。
- `instruction-file-injection`: 同層 AGENTS.md 排在比 CLAUDE.md 更 specific(較高)的位置。

## Impact

- Affected specs: skills-management, instruction-file-injection
- Affected code:
  - Modified:
    - crates/fleety-server/src/skill_sources.rs — 每層來源對順序改 .agents 先;更新測試斷言
    - crates/fleety-server/src/instructions.rs — 每個專案層改 CLAUDE.md 先、AGENTS.md 後;更新測試斷言
  - New: (none)
  - Removed: (none)
