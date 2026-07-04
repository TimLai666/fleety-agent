## 1. 純函式 codex_sources 解析 config.toml mcp_servers

- [x] 1.1 測試先行:在新模組 codex_sources 加 `parse_codex_mcp_from_toml`(給 `[mcp_servers.x]` 帶 command/args 的已解析 TOML,回對應 McpServer;缺 command 的略過)與 `collect_codex_mcp_is_best_effort`(給不存在路徑 / 壞 TOML 回空、不 panic),先紅。驗證:`cargo test -p fleety-server codex_sources` 先紅。
- [x] 1.2 為 fleety-server 加 `toml` 依賴(workspace 已有),實作 codex_sources:`parse_codex_mcp(&toml::Value)` 純函式(取 mcp_servers table → `crate::plugin_sources::McpServer`)+ `collect_codex_mcp(user_home)`(讀 `~/.codex/config.toml`,best-effort)——落實「Requirement: Reuse an originating device's Codex declarative resources」與 design「純函式 codex_sources 解析 config.toml mcp_servers」「best-effort 解析,任何缺失/不符就略過」。驗證:1.1 測試轉綠。

## 2. instructions user 全域加入 Codex AGENTS.md

- [x] 2.1 測試先行:更新 `collect_instruction_paths_layers_and_dedupes` 的 user 全域斷言,使其含 `~/.codex/AGENTS.md`(排在 `~/.claude/CLAUDE.md`、`~/.agents/AGENTS.md` 之後),先紅。驗證:`cargo test -p fleety-server collect_instruction_paths_layers_and_dedupes` 先紅。
- [x] 2.2 在 `collect_instruction_paths` 的 user 全域候選加入 `~/.codex/AGENTS.md`——落實「Requirement: The Codex user-global AGENTS.md is injected」與 design「~/.codex/AGENTS.md 加入指令檔 user 全域層」。驗證:2.1 測試轉綠。

## 3. conn 綁定併入 Codex MCP(使用者 scope)

- [x] 3.1 `build_connection_stack` 綁定時(同主機、有 origin)呼叫 `collect_codex_mcp(user_home)`,把結果轉成 `mcp::ServerCfg` append 到既有的 `conversation_mcp`(排在 plugin 之後,即使用者 scope);跨裝置 / 無 origin 不納入——落實「Requirement: Codex config.toml MCP servers are available per-conversation」與 design「Codex MCP 以使用者 scope、per-conversation 併入」。驗證:`cargo test -p fleety-server codex_mcp_in_conversation`(同主機、`~/.codex/config.toml` 宣告一個 server → 該 server 出現在對話 `mcp_list`)。

## 4. 全量驗證

- [x] 4.1 跑全 workspace 測試與 lint,確認無回歸且非測試碼無新 `unwrap_used`/`expect_used`。驗證:`cargo test` 全綠、`cargo clippy --all-targets` 無新警告。
