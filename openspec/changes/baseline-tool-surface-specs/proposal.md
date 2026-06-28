## Why

Fleety 的「規格」目前散落在 docs/（tools.md 工具正典、env.md、prompts/ 系統提示）與 STATUS.md，沒有單一可驗證的真相來源；剛做過的 docs↔code 逐工具稽核也找出多處 drift（已修）。把規格納入 Spectra 管理，讓每個能力有正規化（SHALL/MUST）規格、可用 spectra analyze/validate 持續檢核並與實作對齊，避免再次漂移。

## What Changes

- 在 openspec/specs/ 下建立**能力規格基準**，涵蓋目前**已出貨的 agent 工具面**，以剛對齊的 docs/tools.md（66 個實際工具）為準。
- 將工具依行為分組為 15 個 capability，每個成為一份 spec：filesystem-tools、command-execution、git-inspection、web-and-network、ssh-execution、browser-automation、computer-use、data-analysis、agent-memory、audit-history、device-registry-and-routing、scheduling、skills-management、mcp-servers、knowledge-wiki。
- 規格以**現況實際行為**為準（工具名稱、參數、risk 分級、守門規則、檔案系統 scope），不是願景。
- **純建立規格文件，不改任何應用程式碼。**

## Non-Goals

- 本變更**不**納入下列項目，各自留作後續變更：環境變數規格（env.md → baseline-config-specs）、系統提示（prompts 的 protocol/rules/memory/policy → baseline-prompt-specs）、headroom 上下文壓縮套件（→ baseline-context-compression-spec）、spec-v0 願景/前瞻概念。
- 不重寫或刪除既有 docs/；tools.md 等續存為人類可讀說明，specs 作為正規真相，兩者並行。
- 不更動任何工具的行為、參數或 risk 分級；本變更只是把現況寫成規格。
- 不嘗試一次涵蓋全部子系統的規格（依 SDD 紀律分階段）。

## Capabilities

### New Capabilities

- `filesystem-tools`: 工作區/裝置檔案讀寫（read_file、list_dir、search_files、write_file、edit_file、delete_file、move_file、make_dir、rollback），含預設整碟可達、FLEETY_FS_SCOPE=workspace 沙箱、敏感路徑守門、備份/rollback。
- `command-execution`: run_command（shell 執行、critical-command 守門、track 變更 diff）。
- `git-inspection`: 唯讀 git 檢視（git_status、git_diff、git_log、git_show）。
- `web-and-network`: HTTP/WS/SSE 出口（fetch_url、http_request、ws_call、sse_stream），含 SSRF 守門與具名 cookie jar。
- `ssh-execution`: ssh_exec（系統 ssh、非互動 BatchMode、host 防注入）。
- `browser-automation`: 瀏覽器 CDP（browser_open/navigate/eval/screenshot/close）與 Chrome 自動佈署，跨裝置 via device_exec。
- `computer-use`: 原生桌面控制（computer_screenshot/move/click/type/key/scroll），跨裝置，最後手段介面。
- `data-analysis`: insyra_exec（.isr DSL，Go sidecar，per-session）。
- `agent-memory`: 核心記憶檔（memory_read/write/edit，ME/USER/TODO/TOOLS）。
- `audit-history`: history_list（每裝置稽核紀錄）。
- `device-registry-and-routing`: 裝置與場域註冊與跨裝置分派（device_list/show/exec/set_site/set_mobility、site_set/list/show/delete、pair_create），含 handle device-scoping。
- `scheduling`: 自管排程（schedule_create/list/delete，mandate-at-creation、fire-time 嚴格比對）。
- `skills-management`: 技能三層（builtin/authored/installed）與檔案級管理（list_skills、use_skill、skill_install/remove/list_files/read_file/write_file/edit_file/delete_file）。
- `mcp-servers`: MCP 子系統（mcp_list/add/remove/call），合併 **builtin 層**（runtime seed 的 builtin.json，含 ddgs 網路搜尋）與 **installed 層**（使用者新增的 installed.json）。
- `knowledge-wiki`: 知識 wiki（wiki_write/read/list/search）與本地 EmbeddingGemma 語意搜尋（wiki_semantic_search）。

### Modified Capabilities

(none)

## Impact

- Affected specs: 15 new capability specs under openspec/specs/ (one spec.md per capability listed above).
- Affected code:
  - New: none (specs/documentation only; spec files are created under openspec by Spectra)
  - Modified: none
  - Removed: none
- Source of truth for the baseline: docs/tools.md (the just-audited canonical tool surface) plus the actual ToolSpec definitions in crates/fleety-tools and crates/fleety-server.
