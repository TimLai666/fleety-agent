## Context

相容層前三步（instruction 檔注入、Claude 外掛 skills/MCP 復用、Codex MCP 復用）已讓 Fleety 對話沿用發起端裝置的宣告式設定。缺口在 hooks：Claude Code 使用者常在 `~/.claude/settings.json`（使用者級）與專案 `.claude/settings.json`（專案級）設定 `PreToolUse`/`PostToolUse` hooks，在工具執行前後跑 shell command 做 lint、格式化、稽核或攔截危險操作。這些 hooks 在 Fleety 對話中完全不生效。

與前三步不同，hooks 會「自動執行任意 shell」，故安全模型是本設計的核心。使用者已拍板兩個開放取向：hooks 預設開（opt-out）、跨裝置也要生效。本設計在其上加一道供應鏈防護：專案級 hooks 可單獨關閉且一律標記來源。

現況可復用的基礎設施：conn 層已在每連線建立 per-conversation 工具 registry（前三步已加 `conversation_sources` / `conversation_mcp` 參數）；`ApprovalGate` 已是工具否決的既有機制；audit log 已記錄工具執行；`device_exec` 已能把命令送到指定裝置的 origin 端執行。

## Goals / Non-Goals

**Goals:**

- 解析發起端 Claude Code `settings.json` 的 `PreToolUse`/`PostToolUse` hooks（使用者級 + 專案級），best-effort。
- 對話工具執行前跑匹配的 PreToolUse hooks、執行後跑 PostToolUse hooks，不動 agent-core 核心。
- hook 於 origin device 執行：同主機本機 shell、跨裝置經 `device_exec`。
- PreToolUse 非零 exit → 比照 `ApprovalGate` 否決工具。
- 每次 hook 執行寫入 audit，專案級標記 `project-sourced`。
- opt-out 安全模型：使用者級預設開；專案級預設開但 `FLEETY_DISABLE_PROJECT_HOOKS=1` 可單獨關閉。

**Non-Goals:**

- 不做 `UserPromptSubmit`/`Stop`/`SubagentStop`/`SessionStart` 等事件。
- 不做 Codex hooks。
- 不自建 hook 安裝/UI，不讓 agent 產生 hook。
- 不做進階 matcher 語法（正規式、工具輸入條件），首版僅工具名精確或 `*` 萬用比對。
- 不保證與 Claude Code hook 執行協定逐位相容（stdin JSON schema、exit code 細節、輸出格式）。

## Decisions

### 決策一：hook 引擎放在 conn 層 tool-wrapper，不動 agent-core

沿用前三步的分層原則——agent-core 不依賴任何 Fleety crate、不含相容邏輯。conn 在組出該對話的工具 registry 後，用一層 wrapper 包住每個工具的 `call`：wrapper 在委派給內層工具前跑 PreToolUse hooks，回傳後跑 PostToolUse hooks。wrapper 持有該對話解析出的 hook 清單、origin 資訊（判斷本機/跨裝置）、audit 匯出點。

替代方案（agent-core 加 observer hook 介面）被否決：會把相容語意滲進核心、且工具執行點分散（前三步經驗顯示 build_full_registry 有多個呼叫點），wrapper 在單一組裝點包住最乾淨。

實作落點（相對規劃的細化，語意不變）：`HookedTool` wrapper 型別與 runner/audit trait 放在 `hooks_compat`（純、可注入假物件測試），conn 只提供正式 runner/audit 與在對話綁定點包裝 registry。包裝用 agent-core `ToolRegistry` 新增的中性 `drain()`（取出既有工具再逐一 re-register 包裝後版本）——這是不帶任何 hook 語意的一般存取器，故不違反「agent-core 不含相容邏輯」；也因此不需在 `build_full_registry` 各呼叫點加參數。首版只包裝主對話綁定點的 registry；crash 復原（recover_incomplete_turn）與 subagent 另建的 registry 不套 hooks，屬後續範圍。

### 決策二：hooks_compat 為純函式解析模組

新模組 `hooks_compat.rs` 只做解析與比對，無 I/O 副作用（讀檔的薄殼另計），比照 `plugin_sources.rs`/`codex_sources.rs`。

- `parse_hooks(&serde_json::Value, scope) -> Vec<HookEntry>`：讀 `hooks.PreToolUse` 與 `hooks.PostToolUse` 陣列；每個陣列元素含 `matcher`（字串，預設 `*`）與 `hooks`（陣列，每項取 `type=="command"` 的 `command` 字串）。展平成 `HookEntry { event, matcher, command, scope }`。
- `matches(matcher, tool_name) -> bool`：`*` 或空字串匹配全部；否則工具名精確比對。
- `collect_hooks(project_cwd, user_home) -> Vec<HookEntry>`：讀使用者 `~/.claude/settings.json` 與專案 `.claude/settings.json`，各自解析並標 scope，best-effort（缺檔/壞 JSON/欄位缺 → 略過）。

`HookEvent` 列舉 `PreToolUse`/`PostToolUse`。`HookScope` 列舉 `User`/`Project`（沿用既有 scope 概念）。

### 決策三：hook 於 origin device 執行

hook 屬於發起端裝置、依賴其檔案系統與工具鏈，須在該環境跑。

- 同主機（binding 判定 origin 即本機）：本機 shell 執行 command。
- 跨裝置：經 `device_exec` 的命令執行把 command 送到 origin device 跑。

沿用 `session-workspace` 的 `WorkspaceBinding` 判斷同主機/跨裝置，與 instruction 檔跨裝置讀取（前一步）同路徑。

### 決策四：PreToolUse 否決走既有 ApprovalGate 語意

PreToolUse hook command 非零 exit 視為否決：wrapper 不呼叫內層工具，回傳一則「被 hook 拒絕」的工具結果給 agent，比照 `ApprovalGate` 拒絕時的結果形狀。首版以 exit code 為準（非零=否決）；stdin 傳入格式與進階 deny 輸出協定列 Open Question。PostToolUse 不否決（工具已執行），失敗只記 audit 警告。

### 決策五：opt-out 安全模型與專案 hooks 供應鏈防護

- 使用者級 hooks：預設開，隨對話生效。
- 專案級 hooks：預設開，但 `FLEETY_DISABLE_PROJECT_HOOKS=1` 時整批不載入。
- 每次 hook 執行都寫 audit；專案級標 `project-sourced`，讓稽核能區分「來自可能不可信 repo」的執行。

理由：專案 `.claude/settings.json` 可能來自 clone 的第三方 repo，等同讓對話自動跑他人指定的 shell（供應鏈風險）。使用者選了 opt-out + 跨裝置這組最開放設定，故加一個可獨立關閉專案 hooks 的開關與強制來源標記，作為最小防護。

### 決策六：best-effort 失敗處理

hook 執行失敗、逾時、或 command 空 → 記 audit 警告後略過，不中斷對話（唯一例外是 PreToolUse 非零 exit 的「刻意否決」，那是預期行為）。

## Implementation Contract

**Behavior（使用者可觀察）：**

- 發起端在 `~/.claude/settings.json` 設 PreToolUse matcher=`Bash` command 於某工具前跑；Fleety 對話中呼叫對應工具時，該 command 先在 origin device 執行，audit 出現一筆該執行紀錄。command 非零 exit → 工具不執行，agent 收到被拒訊息。
- 專案 `.claude/settings.json` 設 hooks 且 `FLEETY_DISABLE_PROJECT_HOOKS=1` → 該批 hooks 不生效；未設該 env → 生效且 audit 標 `project-sourced`。
- PostToolUse hook 在工具回傳後執行；失敗僅記 audit 警告，工具結果照常回給 agent。
- 缺 settings.json 或內容壞掉 → 對話正常進行，無 hook 生效、無崩潰。

**新增/修改介面：**

- 新檔 `crates/fleety-server/src/hooks_compat.rs`：`HookEvent`、`HookScope`、`HookEntry { event, matcher, command, scope }`、`parse_hooks`、`matches`、`collect_hooks`。全 `pub(crate)` 或 `pub` 依 conn 需要。
- `crates/fleety-server/src/main.rs`：宣告 `mod hooks_compat;`。
- `crates/fleety-server/src/conn.rs`：在該對話工具 registry 組裝點插入 tool-wrapper 執行點；新增執行 hook 的私有函式（判本機/跨裝置、跑 shell、彙整 exit、寫 audit）；PreToolUse 否決串到既有拒絕工具結果路徑。

**驗證目標（測試名／可觀察行為）：**

- `hooks_compat` 單元測試：`parse_pretooluse_and_posttooluse`、`parse_is_best_effort_on_bad_json`、`matcher_wildcard_and_exact`、`collect_hooks_tags_scope`。
- conn wrapper 測試（可用假工具 + 假 hook runner 注入）：`pretooluse_nonzero_exit_denies_tool`、`posttooluse_failure_does_not_block`、`project_hooks_disabled_by_env`、`hook_execution_is_audited_with_scope`。

**In scope：** hooks_compat 解析/比對、conn wrapper 執行點、本機/跨裝置執行、PreToolUse 否決、audit、opt-out env。

**Out of scope：** 其他 hook 事件、Codex hooks、進階 matcher、hook 協定逐位相容、hook 安裝 UI。

## Open Questions

- Claude Code hook 執行協定細節：stdin 傳入的 JSON schema（工具名、工具輸入）、exit code 與 deny 的精確語意、hook stdout/stderr 的處理與回傳格式。首版以「非零 exit = 否決、stdin 傳入工具名與序列化輸入」為對映假設，須對實際 Claude Code 版本實測校準。若協定有出入，屬 best-effort 落差，於後續迭代修正而非阻擋首版。
- 跨裝置 hook 執行的逾時與環境（工作目錄、環境變數）對映：首版以 origin cwd 為工作目錄，逾時沿用 `device_exec` 既有上限，細節待實測。
