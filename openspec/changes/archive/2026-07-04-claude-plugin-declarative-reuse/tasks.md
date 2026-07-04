## 1. 純函式 plugin_sources:解析 enabledPlugins、定位已啟用 plugin、best-effort

- [x] 1.1 測試先行:在新模組 plugin_sources 加 `parse_enabled_plugins_handles_object_and_array`(物件形取 value=true 者、陣列形、缺 enabledPlugins 回空)、`collect_plugin_sources_is_best_effort`(給不存在路徑 / 壞 JSON 回空、不 panic)、`collect_plugin_sources_tags_scope`(專案 enabled plugin 的 skill dir 標 Project、使用者的標 User),先紅。驗證:`cargo test -p fleety-server plugin_sources` 先紅。
- [x] 1.2 實作 plugin_sources 模組:`parse_enabled_plugins(&Value)` 純函式 + `collect_plugin_sources(project_cwd, user_home)`(讀專案/使用者 settings、於 plugins 目錄定位 enabled plugin、輸出 `PluginSources { skill_dirs, mcp_servers }` 帶 `Scope`)、全程 best-effort 略過缺失/壞格式——落實「Requirement: Discover and reuse enabled Claude Code plugins」與 design「純函式 plugin_sources 解析 enabledPlugins 並定位已啟用 plugin」與「best-effort 解析,任何缺失/不符就略過該來源、不阻斷」。驗證:1.1 測試轉綠。

## 2. plugin MCP 格式轉換純函式

- [x] 2.1 測試先行:加 `to_fleety_mcp_converts_shape`,斷言 Claude Code 的 `{ name: { command, args } }` 轉成 Fleety 的 `{ name, command, args }` 清單(欄位/順序正確、缺 command 略過),先紅。驗證:`cargo test -p fleety-server to_fleety_mcp_converts_shape` 先紅。
- [x] 2.2 實作 `to_fleety_mcp` 純函式做上述轉換(env 首版不帶)——落實 design「plugin MCP servers per-conversation 併入 mcp register,依 scope」的轉換部分。驗證:2.1 測試轉綠。

## 3. mcp::register 加對話級 server 來源,plugin MCP 併入清單

- [x] 3.1 讓 `mcp::register` 新增「對話級 server 來源」參數(已排序的 Fleety server 清單),併入 `mcp_list` / `mcp_call` 可見的 server 集合,同名 precedence 為 專案 plugin > 使用者 plugin > installed > builtin;更新呼叫點(build_full_registry、scheduler、subagent)補傳空或算好的清單——落實「Requirement: Enabled plugin MCP servers are available per-conversation」與 design「plugin MCP servers per-conversation 併入 mcp register,依 scope」。驗證:`cargo test -p fleety-server` 新增測試斷言傳入的對話級 server 出現在 `mcp_list`、且同名蓋過 installed。

## 4. conn 綁定納入 plugin skills + MCP(同主機,依 scope + precedence)

- [x] 4.1 `build_connection_stack` 綁定時(同主機、有 origin)呼叫 `collect_plugin_sources`:把 plugin 的 skill_dirs 併入該對話的對話級 skill 來源(依 scope,排在同 scope 直接放的 `.agents`/`.claude` skills 之後)、把 mcp_servers 傳給 `mcp::register`;跨裝置 / 無 origin 不納入 plugin 來源——落實「Requirement: Enabled plugin skills join the conversation-scoped tiers」與 design「plugin skills 沿用對話級 skill 來源機制,依 scope 融入」與「precedence 全套 agents-over-claude-precedence」。驗證:`cargo test -p fleety-server plugin_skill_and_mcp_join_conversation`(同主機、啟用一個帶 skill+MCP 的假 plugin → 該 skill 出現在 list_skills、該 server 出現在 mcp_list;跨裝置則否)。

## 5. 全量驗證

- [x] 5.1 跑全 workspace 測試與 lint,確認無回歸且非測試碼無新 `unwrap_used`/`expect_used`。驗證:`cargo test` 全綠、`cargo clippy --all-targets` 無新警告。
