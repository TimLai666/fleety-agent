## Why

conversation-scoped-skills 讓對話能用發起端 `.claude`/`.agents` 直接放的 skills,但使用者在 Claude Code 裝的 **plugin**(打包了 skills、MCP servers 等)裡的資源還用不到。使用者要的是**復用已裝的 plugin**,而非讓 Fleety 自建一套 plugin。plugin 的宣告式資源(skills、MCP servers)可以低風險地讀進來復用,讓 Fleety 對話「像 Claude Code 一樣」用到那些 plugin 帶的能力。hooks(執行式)與 Codex 是後續。

## What Changes

- 新純函式模組 **plugin_sources**:輸入 project cwd 與 user home,解析專案 `.claude/settings.json` 與使用者 `~/.claude/settings.json` 的 `enabledPlugins`,定位 `~/.claude/plugins` 下對應的 **已啟用** plugin 目錄,輸出兩類來源並依 scope(專案/使用者)標記:(a) 每個 enabled plugin 的 `skills/` 目錄;(b) 每個 enabled plugin 的 MCP server 設定(Claude Code `{name:{command,args,env}}` 轉成 Fleety 的 `{name,command,args}`)。解析為 **best-effort**:settings 缺失/格式不符/目錄不存在 → 略過該來源,不阻斷對話。
- **skills**:plugin 的 `skills/` 目錄併入 conversation-scoped-skills 的 skill 來源(依 scope),沿用既有對話級 tier 疊加,零新 skill 機制。
- **MCP**:plugin 的 MCP servers 以 **per-conversation** 方式併入 `mcp::register` 看得到的 server 清單(依 scope),agent 用既有 `mcp_call` 呼叫。
- precedence 全套用 agents-over-claude-precedence(專案 > 使用者 > 全域、`.agents` > `.claude`、直接放的 > plugin 帶的)。
- 只納入 **enabled** 的 plugin;同主機先,跨裝置經 device_exec 讀 plugin 檔列後續;只 Claude Code。

## Non-Goals (optional)

(詳見 design.md;關鍵排除:不做 hooks、不做 Codex、不做 plugin 的 commands/agents、跨裝置 plugin 讀取列後續、不引入 plugin 安裝/自建機制——只讀既有已裝的。)

## Capabilities

### New Capabilities

- `claude-plugin-compat`: 解析發起端 Claude Code 的 `enabledPlugins` 設定、定位已啟用 plugin,並輸出其 skills 目錄與 MCP server 設定供對話復用(依 scope、best-effort)。

### Modified Capabilities

- `skills-management`: 對話級 skill 來源新增「已啟用 plugin 的 skills 目錄」(依 scope 融入)。
- `mcp-servers`: 對話可額外看到「已啟用 plugin 帶的 MCP servers」(per-conversation 併入,依 scope,precedence 高於 installed/builtin)。

## Impact

- Affected specs: claude-plugin-compat(new)、skills-management、mcp-servers
- Affected code:
  - New:
    - crates/fleety-server/src/plugin_sources.rs — 純函式:解析 enabledPlugins、定位 enabled plugin、輸出 skill dirs + MCP configs、best-effort
  - Modified:
    - crates/fleety-server/src/mcp.rs — register 加「對話級 MCP server 來源」參數,併入 server 清單
    - crates/fleety-server/src/conn.rs — build_connection_stack 綁定時算 plugin sources:skills 併入對話級 skill 來源、MCP 併入 register
    - crates/fleety-server/src/scheduler.rs — mcp register 呼叫點補傳空對話級來源
    - crates/fleety-server/src/subagent.rs — 同上(若經過 register)
  - Removed: (none)
