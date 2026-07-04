## Why

Fleety 的 skills 目前全部是全域 server 端的四層 tier(builtin / authored / installed / synced),沒有「跟著發起地點走」的概念。使用者想要的是:在某個專案目錄用 CLI 發起對話,這個對話就能用該處 `.claude/skills`、`.agents/skills` 的專案 skills,以及發起裝置的 user 全域 skills——像 Claude Code 用專案 skills 一樣,且只作用於該對話。

好消息是既有機制幾乎現成:skill 註冊已接受任意目錄清單、`collect` 掃檔合併,tool registry 又是 per-connection 建的。缺的是把「對話級來源目錄」疊進該對話的 registry。但一旦 skill 可能來自別的裝置,`use_skill` / `list_skills` 回傳的 `path` 就成了 protocol.md 明令禁止的「無主 handle」——agent 會拿去在 server 上跑,但檔在別台。所以回傳契約必須新增 skill 所在裝置。

## What Changes

- 新增「對話級 skill 疊加層」:綁定對話時,依 origin 蒐集「發起路徑逐層的 `.claude/skills`、`.agents/skills`(project tier)」與「發起裝置 user 全域 `~/.claude`、`~/.agents` 的 skills(user tier)」,只疊進該對話的 registry。
- precedence:對話級疊在最上層(project > user > installed > authored > builtin > synced)。
- **BREAKING(工具回傳契約)**:`list_skills` 與 `use_skill`(及 skill file 工具的回傳)新增 skill 所在裝置欄位,型別 `Option<String>`(`None` = server 本機,`Some(id)` = 該裝置),與 `WorkspaceBinding.device` 對齊。
- 作用域隔離:對話級 skills 只在該對話可見,不進全域 skill store、不漏到其他對話(靠 per-connection registry 天然成立)。
- **首版只做同主機**(發起端即 server 那台,對話級 skill 的 device 恆為 `None`);跨裝置 skill 來源(經 device_exec 遠端列目錄 / 讀檔)與 scripts 跨裝置執行列後續階段——但回傳契約首版就帶 device 欄位,跨裝置實作時不必再改介面。

## Non-Goals (optional)

(詳見 design.md 的 Goals / Non-Goals;關鍵排除:跨裝置 skill 來源的遠端讀取、scripts 跨裝置執行路由、plugins / hooks。)

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `skills-management`: 新增對話級 project / user skill tier(per-conversation 疊加、precedence 最上、作用域隔離);`list_skills` / `use_skill`(及 skill file 工具回傳)新增 skill 所在裝置欄位。

## Impact

- Affected specs: skills-management(modified);device 表示對齊 session-workspace;與 synced-skill-tier 並存不衝突
- Affected code:
  - New:
    - crates/fleety-server/src/skill_sources.rs — 純函式:依 origin cwd 與 user home 決定對話級 project / user skill 來源目錄(逐層 `.claude/skills`、`.agents/skills` 與 user 全域),回傳有序去重的來源清單
  - Modified:
    - crates/fleety-server/src/skills.rs — SkillInfo 與 skill 來源攜帶 device;register / collect 接受對話級來源目錄並帶出 device;list_skills / use_skill 及 skill file 工具回傳新增 device 欄位
    - crates/fleety-server/src/conn.rs — build_connection_stack 依對話 origin 綁定對話級 skill 來源(首版同主機)
  - Removed: (none)
