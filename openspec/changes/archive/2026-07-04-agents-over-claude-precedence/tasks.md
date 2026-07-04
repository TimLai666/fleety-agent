## 1. skills 同層 .agents 覆蓋 .claude

- [x] 1.1 測試先行:把 `skill_sources_layers_and_dedupes` 的首項斷言由 `.claude/skills` 改為 `.agents/skills`(每層 .agents 排前),並加 `agents_skill_overrides_claude_same_layer`(同一對話級來源目錄下,同名 skill 在 `.agents/skills` 與 `.claude/skills` 都有時,`list_skills`/`use_skill` 服務 `.agents` 版)。此時實作為舊順序,兩測試先紅。驗證:`cargo test -p fleety-server skill_sources agents_skill_overrides_claude_same_layer` 先紅。
- [x] 1.2 把 `skill_sources` 每一層的來源對從 `[.claude/skills, .agents/skills]` 改為 `[.agents/skills, .claude/skills]`(project 各層與 user 全域皆改),使經 `collect_scoped` 反序掃描後 `.agents` 最後掃、最高、覆寫同名 `.claude`——落實「Requirement: Same-layer skill precedence favors .agents over .claude」。驗證:1.1 兩測試轉綠。

## 2. 指令檔同層 AGENTS.md 高於 CLAUDE.md

- [x] 2.1 測試先行:把 `collect_instruction_paths_layers_and_dedupes` 每個專案層的首項斷言由 `AGENTS.md` 改為 `CLAUDE.md`(AGENTS.md 排在該層之後),先紅。驗證:`cargo test -p fleety-server collect_instruction_paths_layers_and_dedupes` 先紅。
- [x] 2.2 把 `collect_instruction_paths` 的每個專案層由 `[AGENTS.md, CLAUDE.md]` 改為 `[CLAUDE.md, AGENTS.md]`(user 全域已是 CLAUDE 先、AGENTS 後,不動),使 AGENTS.md 在該層更 specific(較高)——落實「Requirement: Same-layer instruction precedence favors AGENTS.md over CLAUDE.md」。驗證:2.1 測試轉綠。

## 3. 全量驗證

- [x] 3.1 跑全 workspace 測試與 lint,確認無回歸且非測試碼無新 `unwrap_used`/`expect_used`。驗證:`cargo test` 全綠、`cargo clippy --all-targets` 無新警告。
