## Context

Fleety 的 skills 系統:`skills::register(registry, builtin, authored, installed, synced)` 用四個全域目錄建工具,`collect` 掃這些目錄、以 installed > authored > builtin > synced 合併;`list_skills` / `use_skill` 每次呼叫即時 `collect`。tool registry 由 `build_connection_stack` 每個連線建一份(per-connection)。SKILL.md 是 Agent Skills 標準格式,和 Claude Code `.claude/skills` 同構。

要加的「對話級 skill」擴充點很乾淨:`register`/`collect` 本就吃任意目錄清單,registry 又是 per-connection——把對話級來源只疊進該對話的 registry 即可天然隔離。真正的新問題是:一旦 skill 可能來自別的裝置,`use_skill` 回的 `path` 依 `prompts/protocol.md` 的「no global handles」鐵律不能無主,必須帶所在裝置;否則 agent 會在 server 上對一個位於別台的 path 執行。

本 change 首版只做同主機(對話級 skill 的 device 恆為 server),但把 device 欄位一路穿進資料結構與回傳契約,讓後續跨裝置階段不必再改介面。依賴 session-workspace / origin-injection 提供 origin 定位與 device 表示。

## Goals / Non-Goals

**Goals:**

- 對話能用「發起路徑逐層 `.claude/skills`、`.agents/skills`(project)」與「發起裝置 user 全域(user)」的 skills,疊在既有四層最上。
- 作用域僅限該對話,不進全域、不漏到別對話。
- `list_skills` / `use_skill`(及 skill file 工具)回傳新增 skill 所在裝置,型別對齊 WorkspaceBinding.device。
- 首版同主機即可用;介面契約預先容納跨裝置。

**Non-Goals:**

- 跨裝置 skill 來源的遠端讀取(經 device_exec 列目錄 / 讀 SKILL.md)——列後續階段。
- scripts/ 跨裝置執行路由——沿用 session-workspace 的既有 device 路由,不在此 change 解。
- plugins / hooks(Fleety 無此機制,另議)。
- 不改動既有四層全域 tier 的行為與 skill 生命週期工具語意。

## Decisions

### 對話級 tier 以 per-connection registry 疊加來源目錄

在建該對話的 registry 時,把對話級 project / user 來源目錄額外傳給 skill 註冊,collect 一併掃入。因 registry 是 per-connection,別的對話不傳這些來源即天然隔離,全域 skill store 不動。
替代方案:把對話級 skill 寫進全域 installed tier —— 會外洩到其他對話、且污染使用者的全域 store。故用 per-connection 疊加。

### skill 來源與回傳一路攜帶所在裝置

skill 來源從「純目錄」升級為「(device, 目錄)」,`SkillInfo` 加 `device` 欄位,`collect` / `list_skills` / `use_skill` / skill file 工具把 device 一路帶出到 JSON 回傳(`device`:`null` = server,否則裝置 id)。呼應 protocol.md「no global handles」:回傳的 `path` 必須有裝置歸屬。
替代方案:只回 path 不回 device —— 跨裝置時 path 無主,agent 在錯的裝置執行。故契約首版就帶 device。

### 純函式決定 project 與 user skill 來源目錄

以純函式從 (origin_cwd, user_home) 算出對話級來源目錄:origin cwd 逐層往上,每層取 `.claude/skills` 與 `.agents/skills`(存在者),再加 user 全域 `~/.claude/skills`、`~/.agents/skills`;有序(深→淺,越深優先)且去重。純函式與 I/O、registry 組裝分離,易測。
替代方案:在 conn.rs 內嵌路徑發現 —— 難測、與註冊耦合。故抽純函式。

### precedence:對話級疊在既有四層之上

合併順序讓對話級最高:project > user > installed > authored > builtin > synced。專案 skill 最貼近當前任務,最該優先,呼應既有「more specific 覆寫」。
替代方案:對話級墊底 —— 全域 installed 會蓋掉專案 skill,違背「用專案裡的 skill」的意圖。故疊最上。

### 首版同主機,跨裝置 skill 來源與 scripts 執行列後續

首版只在「發起端 == server 同主機」時蒐集對話級來源(本機 `std::fs`),device 恆為 `None`。跨裝置時對話級來源為空(退回既有四層),留待後續以 device_exec 遠端讀取補上;scripts 跨裝置執行沿用 session-workspace 既有路由。
替代方案:首版就做跨裝置遠端 skill 掃描 —— 成本最高(遠端列目錄 + 讀檔 + 快取),拖累首版。故切為兩階段,介面先容納 device。

## Implementation Contract

**Behavior:** 在專案目錄(同主機)綁定的對話,其 `list_skills` 除既有四層外,還含該處 origin cwd 逐層 `.claude/skills`、`.agents/skills` 的 project skills 與發起裝置 user 全域 skills,precedence 疊最上;每個 skill 條目與 `use_skill` 回傳含所在裝置(首版恆為 server)。另一個綁定到不同專案的對話看不到前者的對話級 skills。

**Interface / data shape:** 純函式 `skill_sources(origin_cwd, user_home)` 回傳有序去重的來源目錄清單。skill 來源型別攜帶 `device: Option<String>`(`None` = server)。`SkillInfo` 加 `device`。`list_skills` 每條目與 `use_skill`、`skill_list_files` / `skill_read_file` 等回傳的 JSON 新增 `device` 欄位(`null` 或裝置 id),既有 `name` / `source` / `path` / `content` 欄位不變。

**Failure modes:** 來源目錄不存在 → 略過。同名 skill 跨 tier → 依 precedence 取最高。無 origin / 舊 CLI / 跨裝置(首版)→ 無對話級來源,退回既有四層,不報錯。

**Acceptance criteria:**
- 純函式測試 `skill_sources_layers_and_dedupes`:給定 origin_cwd 與 user_home,回傳逐層 `.claude/skills`、`.agents/skills` + user 全域、有序去重的來源清單。
- 測試 `collect_carries_device`:對話級來源的 skill 帶 `device`(同主機為 `None`)。
- 測試 `list_and_use_report_device`:`list_skills` 每條目與 `use_skill` 回傳含 `device` 欄位。
- 測試 `conversation_scoped_skill_is_isolated`:兩個綁定不同來源的 registry,各自只見自身對話級 skill。
- 測試 `conversation_tier_overrides_global`:同名 skill 在對話級與 installed 都有時,對話級勝出。

**Scope 邊界:** in scope —— 來源發現純函式(新模組)、skills.rs 的 device 穿透與對話級來源疊加、conn.rs 綁定時掛對話級來源(同主機)、skills-management spec 的 tier 與 device 契約、上述測試。out of scope —— 跨裝置遠端 skill 讀取、scripts 跨裝置執行、plugins / hooks、既有全域 tier 行為。

## Risks / Trade-offs

- [回傳新增 device 欄位是契約變更] → 採新增欄位、不改既有 `name`/`source`/`path`,消費端相容;更新 protocol.md 指引讓 agent 跑跨裝置 skill 的 scripts 時用該 device。
- [每次 list/use 掃對話級來源目錄的成本] → 來源數量有限(逐層 + user 兩處),與既有 collect 同量級,可接受。
- [首版同主機但介面帶 device 看似多餘] → 這是刻意的:避免跨裝置階段再改一次介面。
- [`.claude`/`.agents` 逐層往上到哪停] → 見 Open Questions;建議與 instruction-file-injection 的「專案根界定」一致。

## Migration Plan

純新增來源發現與 device 欄位,無資料遷移,不改既有全域 tier 的磁碟結構。部署後同主機在專案目錄發起的新對話即獲得對話級 skills。Rollback:移除對話級來源疊加與 device 欄位即可回到四層全域行為。

## Open Questions

- project skill 搜尋範圍:origin cwd 逐層往上直到 user_home / 檔系統根,或只認專案根一層?建議與 instruction-file-injection 一致(逐層往上到 user_home / fs 根),兩者共用同一「往上到哪」的決策。
- user tier 位置:僅 `~/.claude/skills` 與 `~/.agents/skills`,或也含其他?建議首版限這兩處。
- device 欄位命名:建議 `device`(對齊 WorkspaceBinding.device),而非 `device_id`。
