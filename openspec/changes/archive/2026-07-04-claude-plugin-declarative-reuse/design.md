## Context

conversation-scoped-skills 已建立「對話級 skill 來源(一串目錄)→ 疊加到 per-connection registry」的機制;mcp.rs 是「config 清單({servers:[{name,command,args}]},installed shadow builtin)+ `mcp_call`」模型。本 change 讓對話再多讀一個來源:發起端 Claude Code **已啟用 plugin** 帶的 skills 目錄與 MCP servers。

Claude Code plugin:裝在 `~/.claude/plugins/` 下,啟用狀態記在 `settings.json` 的 `enabledPlugins`(使用者 `~/.claude/settings.json`、專案 `.claude/settings.json`)。每個 plugin 目錄可含 `skills/`、`.mcp.json`(或 manifest 的 `mcpServers`)、以及 commands/agents/hooks(不在本 change)。

主要不確定性:Claude Code 的 `enabledPlugins` 與 plugin 目錄佈局是**外部、跨版本可能變**的格式。本 change 一律 **best-effort**:解析已知形狀,任何缺失/不符就略過該來源、不阻斷對話,並把確切格式列為需實測校準的 Open Question。

## Goals / Non-Goals

**Goals:**

- 對話能復用發起端 Claude Code **已啟用** plugin 的 skills 與 MCP servers。
- plugin skills/MCP 依「plugin 裝在哪」(專案 enabled / 使用者 enabled)融入對應 scope,套 agents-over-claude-precedence。
- 解析全程 best-effort,不因 Claude Code 格式差異阻斷對話。

**Non-Goals:**

- 不做 hooks(執行式,④)、不做 Codex(③)。
- 不做 plugin 的 commands / agents(只 skills + MCP)。
- 跨裝置讀 plugin 檔列後續(同主機先)。
- 不引入 plugin 安裝/自建;只讀既有已裝的。
- 不改既有 skill tier / MCP 的核心行為,只多加來源。

## Decisions

### 純函式 plugin_sources 解析 enabledPlugins 並定位已啟用 plugin

新模組把工作拆成可測的純函式 + 一層 best-effort I/O:
- `parse_enabled_plugins(settings: &Value) -> Vec<String>`:從一份已讀入的 settings JSON 取 `enabledPlugins`,容忍兩種常見形狀——物件 `{ "name": true/false, … }`(取值為 true 者)與陣列 `[ "name", … ]`。純函式,不碰檔案。
- `collect_plugin_sources(project_cwd, user_home) -> PluginSources`:讀專案與使用者 settings(best-effort)、對每個 enabled plugin 於 `~/.claude/plugins` 定位目錄,產出 `PluginSources { skill_dirs: Vec<(Scope, PathBuf)>, mcp_servers: Vec<(Scope, McpServer)> }`。任何一步失敗都略過該來源。
`Scope` 為 `Project` / `User`,對映 agents-over-claude-precedence 的 scope 層。
替代方案:把解析內嵌 conn.rs —— 難測、與綁定耦合、格式脆點藏在整合裡。故抽模組 + 純解析可測。

### plugin skills 沿用對話級 skill 來源機制,依 scope 融入

plugin 的 `skills/` 目錄就是額外的 skill source dir。把它們接到 conn 綁定時算出的對話級 skill 來源清單裡(專案 plugin 的排在專案 scope、使用者 plugin 的排在使用者 scope),交給既有 `collect_scoped` 疊加。零新 skill 機制。
替代方案:為 plugin skills 造獨立 tier —— 重複 conversation-scoped-skills 已有的疊加,無必要。

### plugin MCP servers per-conversation 併入 mcp register,依 scope

`mcp::register` 目前簽章 `(registry, builtin, installed)`。加一個「對話級 server 來源」參數(依 scope 排好、已轉成 Fleety `{name,command,args}` 的清單),併入 `mcp_list`/`mcp_call` 看得到的 server 集合,同名以高 scope 覆蓋(專案 plugin > 使用者 plugin > installed > builtin)。轉換用純函式 `to_fleety_mcp(claude_obj)`:Claude Code 的 `{ name: { command, args, env } }` → Fleety 的 `{ name, command, args }`(env 目前不帶,列 Open Question)。
替代方案:把 plugin MCP `mcp_add` 進全域 installed.json —— 變全域、非 per-conversation,失去「復用發起端」語意且污染使用者全域設定。

### best-effort 解析,任何缺失/不符就略過該來源、不阻斷

settings 檔不存在、JSON 壞、`enabledPlugins` 缺、plugin 目錄不在、`.mcp.json` 壞 → 該來源略過,對話照常。這是面對「外部跨版本格式」的唯一安全姿態。

### precedence 全套 agents-over-claude-precedence

skills 與 MCP 的 scope 排序、`.agents` > `.claude`、直接放的 > plugin 帶的,一律沿用 ① 定的規則。plugin 帶的 skills 排在同 scope 內「直接放的 `.agents`/`.claude` skills」之後(較低)。

## Implementation Contract

**Behavior:** 對話綁定時(同主機),runtime 讀發起端 Claude Code 的專案與使用者 `settings.json`,對每個 **enabled** plugin,把其 `skills/` 併入該 scope 的對話級 skill 來源、其 MCP servers 併入該對話 `mcp_list`/`mcp_call` 可見的 server 清單。任何解析步驟失敗就略過該來源。跨裝置 / 無 origin → 不納入 plugin 來源(退回既有行為)。

**Interface / data shape:**
- `parse_enabled_plugins(&Value) -> Vec<String>`(純;容忍 object/array)。
- `to_fleety_mcp` 純函式:Claude Code MCP object → Fleety `{name,command,args}` 清單。
- `PluginSources { skill_dirs: Vec<(Scope, PathBuf)>, mcp_servers: Vec<(Scope, McpServer)> }`;`Scope::{Project,User}`。
- `mcp::register` 新增對話級 server 來源參數(型別為已排序的 Fleety server 清單);既有 `(builtin, installed)` 語意不變。呼叫點(conn `build_full_registry`、scheduler、subagent)補傳空清單或算好的清單。

**Failure modes:** 缺 settings / 壞 JSON / 缺 `enabledPlugins` / plugin 目錄不存在 / 缺 skills 目錄 / 壞 `.mcp.json` → 略過該來源,回傳空或部分結果,不 panic、不阻斷對話。MCP server 的 command 不在 PATH → 沿用既有 `mcp_call` 的錯誤路徑。

**Acceptance criteria:**
- `parse_enabled_plugins_handles_object_and_array`:物件形(取 true 者)與陣列形都回正確名單;缺欄位回空。
- `to_fleety_mcp_converts_shape`:Claude Code `{name:{command,args}}` → Fleety `{name,command,args}` 清單,順序/欄位正確。
- `collect_plugin_sources_is_best_effort`:給不存在的路徑 / 壞 JSON,回空、不 panic。
- `collect_plugin_sources_tags_scope`:專案 enabled plugin 的 skill dir 標 `Project`、使用者的標 `User`。
- 整合:同主機、專案啟用一個帶 skill 的 plugin → 該 plugin 的 skill 出現在對話 `list_skills`;帶 MCP 的 plugin → 該 server 出現在 `mcp_list`。

**Scope 邊界:** in scope —— plugin_sources 模組、mcp::register 加對話級來源參數、conn 綁定納入 plugin skills + MCP、上述測試。out of scope —— hooks、Codex、commands/agents、跨裝置 plugin 讀取、plugin 安裝、既有 tier/MCP 核心行為。

## Risks / Trade-offs

- [Claude Code 的 enabledPlugins / plugin 佈局跨版本變] → 全程 best-effort、只解析已知形狀、失敗略過;確切格式列 Open Questions 待實測校準。
- [plugin MCP 的 command 不在 server 主機的 PATH] → mcp_call 既有錯誤路徑處理,不 panic。
- [解析成本(每次綁定讀 settings + 列 plugin 目錄)] → 綁定時算一次(非每輪);同主機 fs,可接受。
- [env 未帶入 MCP 轉換] → 首版不帶 env,列 Open Question;多數 stdio MCP 不需額外 env。

## Migration Plan

純新增來源與一個 register 參數,無資料遷移。部署後同主機、已啟用 plugin 的對話即獲得其 skills/MCP。Rollback:移除 plugin_sources 併入點與 register 新參數即可。

## Open Questions

- Claude Code `enabledPlugins` 的確切格式與 plugin 目錄佈局(`~/.claude/plugins` 下的 marketplace/name 結構)——首版解析常見形狀 + best-effort,實裝一個 plugin 後校準。
- plugin MCP 是否需要帶 `env` / `cwd`?首版只帶 command+args;若實測有 plugin 依賴 env,再擴充轉換。
- 專案 `.claude/settings.json` 的 enabledPlugins 是否與使用者級語意一致(啟用 / 停用覆蓋)?best-effort 先各自取 enabled 聯集,實測校準。
