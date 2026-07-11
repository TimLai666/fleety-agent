## 1. 探索規則改寫（TDD）

- [x] 1.1 先紅：在 crates/fleety-server/src/skill_sync.rs 測試模組新增規格「The synced tier mirrors the repo's skills, additions and removals」修訂後的行為測試，測試值沿用 delta spec 的 Example 表格：(a) plugin 佈局 plugins/p1/skills/b/SKILL.md 被探索為 skill b；(b) 巢狀 sub-skill（myskill/SKILL.md 之下的 myskill/subskill/SKILL.md）不拆分，subskill 以原相對路徑隨父目錄進入 synced tier；(c) 跨路徑同名 skill 依相對路徑排序先到先贏、其餘跳過；(d) 舊平面佈局回歸（頂層 a/SKILL.md 仍探索為 a）；(e) dot 目錄（.claude-plugin/x/SKILL.md）與 repo root 的 SKILL.md 都忽略。驗證：cargo test -p fleety-server skill_sync 出現預期的失敗（紅）。
- [x] 1.2 實作剪枝式遞迴探索並轉綠：skill 識別維持純函式，改為回傳能定位任意深度 skill 根的形狀（相對路徑集合）；rebuild 複製迴圈依葉目錄名攤平寫入 staging，撞名時先到先贏並記 warning log，同步不失敗。行為契約：三種佈局（平面、plugin marketplace、含巢狀 sub-skill）探索結果符合 delta spec Example 表格，synced tier 產出仍為攤平的一層 skill 目錄。驗證：1.1 全部轉綠，且既有 skill_sync 測試（rebuild_and_swap_mirrors_additions_and_removals、skill_dirs_only_top_level_with_skill_md 的對應更新版）不回歸，cargo test -p fleety-server 綠。

## 2. 文件與真實 repo 驗證

- [x] 2.1 同步文件描述：skill_sync.rs 頂部 module doc 中「top-level skill directories」的措辭改為剪枝式遞迴規則的描述；核對 docs/env.md 的 FLEETY_SKILLS_SYNC* 條目（env 面無行為變更，僅在提及佈局處校正措辭）。驗證：內容審閱兩處措辭與 delta spec 一致，cargo test -p fleety-server 綠。
- [x] 2.2 對真實來源 repo 實測：下載 TimLai666/skills 的 main 分支 zip 走一次完整 stage 流程（真實網路、真實佈局），確認 synced 產出包含 plugins 底下全部 skills（數量與抽樣名稱人工核對，例如 plugins/dev-workflow/skills 與 plugins/knowledge-tools/skills 內的目錄名都出現）、無 .claude-plugin 或 .obsidian 汙染、且任一含巢狀 SKILL.md 的 skill 保持整包未拆。驗證：列出產出目錄清單人工核對。
