## Context

② 建立了「對話級 MCP server 來源(per-conversation,依 scope)+ plugin skills 併入對話級 skill 來源」的機制;instruction-file-injection 有「user 全域指令檔」層。本 change 把同一套延伸到 Codex:Codex 的 MCP servers 在 `~/.codex/config.toml`(TOML `[mcp_servers.<name>]`),指令檔在 `~/.codex/AGENTS.md`。兩者都是使用者級、宣告式,正好套現有機制。

Codex 沒有 Claude Code 那種 plugin 打包 / hooks;它的宣告式資源就是 config.toml 的 MCP 與 AGENTS.md。所以本 change 比 ② 小:一個 TOML 解析 + 兩個既有機制的接點。

不確定性同 ②:Codex 檔案格式受版本影響,一律 best-effort。

## Goals / Non-Goals

**Goals:**

- 對話復用發起端 Codex 的 MCP servers(config.toml)與 `~/.codex/AGENTS.md`。
- MCP 走 ② 的對話級 server 機制(使用者 scope);AGENTS.md 走 instruction-file-injection 的 user 全域層。
- best-effort、同主機首版。

**Non-Goals:**

- 不做 hooks、Codex prompts / custom commands(slash 概念與 skills 不對映)、Codex skills(格式未定)。
- 跨裝置讀 Codex 檔列後續。
- 不引入 Codex 安裝機制;只讀既有設定。

## Decisions

### 純函式 codex_sources 解析 config.toml mcp_servers

新模組把「解析」與「I/O」分開:`parse_codex_mcp(config: &toml::Value) -> Vec<McpServer>` 純函式,從已解析的 TOML 取 `mcp_servers` table,每個 key 是 server 名、value 取 `command`/`args`,缺 command 略過;`collect_codex_mcp(user_home) -> Vec<McpServer>` 讀 `~/.codex/config.toml`(best-effort)並呼叫純函式。復用 `crate::plugin_sources::McpServer` 型別,讓 conn 用同一條路轉成 `mcp::ServerCfg`。用 workspace 既有的 `toml` crate(fleety-tools 已用,fleety-server 加為依賴)。
替代方案:手寫 TOML 解析 —— 脆、易錯;用既有 toml crate 標準且已在 workspace。

### Codex MCP 以使用者 scope、per-conversation 併入

Codex 是使用者級設定,其 MCP servers 歸使用者 scope,和 ② 的「使用者 plugin MCP」同層。conn 綁定時(同主機)把 `collect_codex_mcp` 的結果轉 `ServerCfg` 併入既有的 `conversation_mcp`,排在 plugin 之後(precedence:專案 plugin > 使用者 plugin > Codex > installed > builtin)。同名以較高者為準(load_merged 既有邏輯)。
替代方案:把 Codex MCP 當全域 installed —— 變全域、非 per-conversation,失去「復用發起端」語意。

### ~/.codex/AGENTS.md 加入指令檔 user 全域層

`collect_instruction_paths` 的 user 全域目前收 `~/.claude/CLAUDE.md`、`~/.agents/AGENTS.md`;再加 `~/.codex/AGENTS.md`。它和其他 user 全域一樣是軟疊加(deeper/後者更 specific),排在 user 層。
替代方案:另做 Codex 專屬注入 —— 重複既有 user 全域機制,無必要。

### best-effort 解析,任何缺失/不符就略過

`~/.codex/config.toml` 不存在、TOML 壞、無 `mcp_servers`、`~/.codex/AGENTS.md` 不存在 → 略過該來源,對話照常。面對外部跨版本格式的唯一安全姿態。

## Implementation Contract

**Behavior:** 同主機對話綁定時,runtime 讀 `~/.codex/config.toml`,把其中宣告的 MCP servers 併入該對話的 MCP server 清單(使用者 scope、per-conversation、可 `mcp_call`);並把 `~/.codex/AGENTS.md` 併入該對話的 user 全域指令檔注入。任何解析失敗略過。跨裝置 / 無 origin 不納入 Codex 來源。

**Interface / data shape:** `parse_codex_mcp(&toml::Value) -> Vec<crate::plugin_sources::McpServer>`(純)。`collect_codex_mcp(user_home: &Path) -> Vec<McpServer>`(best-effort I/O)。`collect_instruction_paths` 的 user 全域候選新增 `~/.codex/AGENTS.md`。conn 綁定把 Codex servers 轉 `mcp::ServerCfg` append 到既有 `conversation_mcp`。

**Failure modes:** 缺 config.toml / 壞 TOML / 無 mcp_servers / 缺 AGENTS.md → 略過,回空或部分,不 panic、不阻斷。MCP command 不在 PATH → 既有 mcp_call 錯誤路徑。

**Acceptance criteria:**
- `parse_codex_mcp_from_toml`:`[mcp_servers.x]` 帶 command/args 的 TOML → 對應 McpServer;缺 command 的略過。
- `collect_codex_mcp_is_best_effort`:不存在路徑 / 壞 TOML → 空、不 panic。
- `codex_agents_md_in_user_global`:`collect_instruction_paths` 的結果含 `~/.codex/AGENTS.md`。
- 整合:同主機、`~/.codex/config.toml` 宣告一個 server → 該 server 出現在對話 `mcp_list`。

**Scope 邊界:** in scope —— codex_sources 模組、conn 綁定併入 Codex MCP、instructions user 全域加 Codex AGENTS.md、fleety-server 加 toml 依賴、上述測試。out of scope —— hooks、Codex prompts/skills、跨裝置、既有機制核心行為。

## Risks / Trade-offs

- [Codex config.toml / AGENTS.md 路徑或格式跨版本變] → 全程 best-effort、失敗略過;確切格式列 Open Questions 實測校準。
- [新增 toml 依賴到 fleety-server] → workspace 已有(fleety-tools 用),只是啟用,低風險。
- [Codex MCP command 不在 server PATH] → 既有 mcp_call 錯誤路徑,不 panic。

## Migration Plan

純新增一個來源解析 + 兩個既有接點,無資料遷移。部署後同主機、有 Codex 設定的對話即獲得其 MCP + AGENTS.md。Rollback:移除 codex_sources 併入點與 instructions 的 Codex 路徑、去掉 toml 依賴。

## Open Questions

- `~/.codex/config.toml` 的 mcp_servers 確切鍵名與結構、`~/.codex/AGENTS.md` 的實際位置 —— 首版解析常見形狀 + best-effort,實裝校準。
- Codex MCP 是否需帶 `env` / 工作目錄?首版只帶 command+args,與 ② 一致;實測有需要再擴充。
