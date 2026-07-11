## Why

skills 來源 repo（預設 TimLai666/skills）已改為 Claude plugin marketplace 佈局：skills 位於 plugins/<plugin>/skills/<skill>/SKILL.md，repo 頂層只剩 marketplace 描述檔與 plugins 目錄。現行 skill sync 的探索規則只認「repo 頂層直接含 SKILL.md 的目錄」（synced-skill-tier 規格明文 top-level），對新佈局會探索到空集合，而「removals are inherent」的原子換入設計會把 synced tier 換成空目錄 —— 不報錯、log 顯示同步成功，實際上靜默清空整層 synced skills。每小時一次的背景同步意味著改版後的 server 都已（或即將）踩到。

## What Changes

- 探索規則從「頂層含 SKILL.md 的目錄」改為**剪枝式遞迴**：由上而下走訪目錄樹（跳過 . 開頭目錄，如 .claude-plugin、.obsidian、.git），第一個含 SKILL.md 的目錄即為 skill 根，整個目錄視為一個 skill 單位，**不再往其內部遞迴**。
- 因此三種佈局通吃：舊平面佈局（頂層 <skill>/SKILL.md）行為不變；plugin marketplace 佈局（plugins/<plugin>/skills/<skill>/SKILL.md）能探索到全部 skills；含巢狀 sub-skill 的目錄（myskill/SKILL.md 之下還有 myskill/subskill/SKILL.md）不會被拆成多個 skill，subskill 隨父目錄整包同步、保持相對位置。
- repo root 本身若含 SKILL.md 則忽略（維持「loose root 內容不是 skill」的既有語義，也避免整個 repo 被當成單一 skill）。
- 跨路徑同名 skill（兩個 plugin 各有同名 skill 目錄）：以相對路徑排序先到先贏，落選者記 warning log，同步不失敗。
- synced tier 產出形狀不變：仍是攤平的 skills/synced/<skill-name>/，skill 內容整目錄複製（references、assets、scripts、巢狀 sub-skill 都跟著走）。

## Non-Goals

- 不解析 marketplace.json 或 plugin.json：那會把 Fleety 耦合到 Claude plugin manifest schema（會演化），而且平面佈局的 repo 仍需要目錄探索當 fallback，等於兩套邏輯。目錄探索一套通吃。
- 不同步 plugin 的 commands、agents、hooks 等非 skill 內容：synced tier 是 skills 專屬。
- 不引入 plugin 層級的啟用/停用或 skill 名稱前綴命名空間：目前來源 repo 的 skill 名全域唯一，撞名以確定性規則加警告處理即可；若未來需要同名異義，再另案設計。
- 不改動同步機制的其他部分：SHA 條件下載、原子換入、never-crash、env 設定面（FLEETY_SKILLS_SYNC*）全部維持。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `synced-skill-tier`: 「The synced tier mirrors the repo's skills」條款的 skill 識別規則，從「top-level directory that contains a SKILL.md」改為「剪枝式遞迴找最外層含 SKILL.md 的目錄」，並新增巢狀 sub-skill 不拆分與跨路徑撞名的行為定義。

## Impact

- Affected specs: synced-skill-tier（MODIFIED）
- Affected code:
  - Modified: crates/fleety-server/src/skill_sync.rs
  - New: 無
  - Removed: 無
