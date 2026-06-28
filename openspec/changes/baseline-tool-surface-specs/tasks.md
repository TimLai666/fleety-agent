<!-- 本變更不寫程式:每個任務是「規格 ↔ 現況程式/文件」的一致性驗收,
     行為 = 對應的 capability 規格與已出貨工具行為相符;
     驗證 = 對照 ToolSpec / 程式位置 / docs/tools.md / 測試 / spectra validate。 -->

## 1. 檔案、指令與 Git 工具驗收

- [ ] 1.1 [P] filesystem-tools 規格 "Read and inspect workspace files"、"Mutate files with backup and rollback"、"Filesystem scope and sensitive-path guard" 與現況相符(read_file 行號/slice、備份+rollback、預設整碟可達 vs FLEETY_FS_SCOPE=workspace 沙箱、敏感路徑寫入守門)。驗證:對照 crates/fleety-tools/src/lib.rs 的 resolve_in_root/guard_sensitive 與 docs/tools.md;cargo test -p fleety-tools 的 scope/sensitive 測試通過。
- [ ] 1.2 [P] command-execution 規格 "Run shell commands with a critical-command guard" 與 "Diff files a command changed" 與現況相符(run_command 參數僅 command/cwd/track、critical 拒絕、track diff)。驗證:對照 run_command ToolSpec 與 docs/tools.md(稽核已將 timeout_secs 改為 cwd)。
- [ ] 1.3 [P] git-inspection 規格 "Read-only git inspection" 與現況相符(git_status/diff/log/show 唯讀,git_diff 無參數且含未追蹤檔)。驗證:對照 ToolSpec —— git_diff parameters 為空(稽核已確認並修正文件)。

## 2. 網路與遠端執行驗收

- [ ] 2.1 [P] web-and-network 規格 "HTTP, WebSocket, and SSE egress with SSRF guard" 與 "Persistent named cookie jars" 與現況相符(fetch_url/http_request/ws_call/sse_stream、SSRF 守門吃 FLEETY_ALLOW_PRIVATE_NET、cookie_jar 跨呼叫持久)。驗證:對照 ToolSpec 與 docs/tools.md、docs/env.md。
- [ ] 2.2 [P] ssh-execution 規格 "Run commands on a remote host over SSH" 與現況相符(ssh_exec 為 host/command + 選填 user/port/identity、BatchMode、host 防注入、無 timeout_secs)。驗證:對照 ssh_exec ToolSpec(稽核已修 identity_file→identity 並移除 timeout_secs)。

## 3. 裝置 UI 控制驗收

- [ ] 3.1 [P] browser-automation 規格 "Drive a browser over the DevTools Protocol on any device" 與 "Auto-provision a local Chrome" 與現況相符(browser_* 於 server 與每台裝置註冊、screenshot 為 Read、Chrome 偵測/啟動/安裝、遠端端點不自動佈署)。驗證:對照 crates/fleety-tools/src/browser.rs 與 chrome.rs 及裝置註冊路徑。
- [ ] 3.2 [P] computer-use 規格 "Native desktop control on any device" 與現況相符(computer_* 原生 enigo/xcap、screenshot 為 Read 其餘 Mutate、headless 給可行動錯誤、modifier 失敗仍釋放)。驗證:對照 crates/fleety-tools/src/computer.rs。

## 4. 記憶、知識與資料分析驗收

- [ ] 4.1 [P] data-analysis 規格 "Run the Insyra data-analysis DSL" 與現況相符(insyra_exec 之 session/command/script/reset、Go sidecar、save 落工作區)。驗證:對照 insyra_exec ToolSpec 與 sidecar 佈署。
- [ ] 4.2 [P] agent-memory 規格 "Read and edit agent core memory files" 與現況相符(memory_read/write/edit、行號+行範圍編輯、不含 device 參數、僅 ME/USER/TODO/TOOLS)。驗證:對照 crates/fleety-server/src/tools.rs 與 docs/tools.md(稽核已移除幻覺 device 參數)。
- [ ] 4.3 [P] audit-history 規格 "List recent audit entries" 與現況相符(history_list 之 limit 預設 20、每工具呼叫入稽核)。驗證:對照 history_list ToolSpec 與 append_history 寫入路徑。
- [ ] 4.4 [P] knowledge-wiki 規格 "Read and write the knowledge wiki" 與 "Local semantic search over the wiki" 與現況相符(wiki_write/read/list/search、wiki_semantic_search top_k/cosine、雜湊變更重嵌、停用時回退錯誤)。驗證:對照 crates/fleety-server/src/wiki_embed.rs 與 FLEETY_WIKI_EMBED 停用回退。

## 5. 編排與裝置註冊驗收

- [ ] 5.1 [P] device-registry-and-routing 規格 "Register devices and sites" 與 "Route a tool call to another device" 與現況相符(device_*/site_*/pair_create;device_exec 對 advertised tools 嚴格比對、handle device-scoping 拒絕跨裝置)。驗證:對照 device_exec 實作(bridge 路徑)與 device_show 的 advertised 工具回傳。
- [ ] 5.2 [P] scheduling 規格 "Schedule prompts to run later" 與現況相符(schedule_create/list/delete、trigger one-shot/cron+tz、建立時擷取 mandate/allowed_tools、list 顯示 tz 與 next_fire)。驗證:對照 schedule ToolSpec 與 schedule_list 輸出。

## 6. 技能與 MCP 子系統驗收

- [ ] 6.1 [P] skills-management 規格 "Three-tier skill store" 與 "File-level skill editing with tier rules" 與現況相符(builtin/authored/installed 合併與優先序、list_skills 回 source+path、skill_* 檔案級、builtin 唯讀、新檔落 authored、in-skill 路徑防逃逸)。驗證:對照 crates/fleety-server/src/skills.rs。
- [ ] 6.2 [P] mcp-servers 規格 "Manage and call MCP servers across builtin and installed tiers" 與 "Built-in ddgs web search" 與現況相符(mcp_list/add/remove/call;builtin.json/installed.json 兩層合併、mcp_add 只寫 installed、mcp_remove 不刪 builtin;ddgs 屬 builtin 層,開機 seed + 自動安裝/升級,經同一 mcp_call 介面存取)。驗證:對照 crates/fleety-server/src/builtin_mcp.rs 的 builtin_servers()/seed 與 docs/env.md 的 ddgs 段。

## 7. 整體驗收與 drift 收斂

- [ ] 7.1 全 15 份能力規格通過 Spectra 結構與 scenario 檢核。驗證:spectra validate baseline-tool-surface-specs 零錯誤。
- [ ] 7.2 確認上述 15 能力的 normative 主張無一與現況 codebase / docs/tools.md 衝突;發現的差異只記錄為後續 change(env-config / system-prompt / context-compression / spec-v0),本變更不改程式。驗證:逐能力打勾完成,且後續變更已列於 proposal 的 Non-Goals。
