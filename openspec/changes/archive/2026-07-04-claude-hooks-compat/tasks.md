## 1. hooks_compat 解析模組（純函式）— 決策二：hooks_compat 為純函式解析模組；需求 Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks

- [x] 1.1 建立 `crates/fleety-server/src/hooks_compat.rs` 並在 `crates/fleety-server/src/main.rs` 加 `mod hooks_compat;`；定義 `HookEvent`（`PreToolUse`/`PostToolUse`）、`HookScope`（`User`/`Project`）、`HookEntry { event, matcher, command, scope }`
- [x] 1.2 (紅) 寫測試 `parse_pretooluse_and_posttooluse`：給一段含 `hooks.PreToolUse` 與 `hooks.PostToolUse`（各含 `matcher` 與 `hooks[].command`，`type=="command"`）的 `serde_json::Value`，斷言 `parse_hooks` 展平出正確 event/matcher/command/scope
- [x] 1.3 (綠) 實作 `parse_hooks(&serde_json::Value, scope: HookScope) -> Vec<HookEntry>`：讀兩個事件陣列，取 `type=="command"` 的 `command`，`matcher` 缺省視為 `*`，讓 1.2 通過（實現需求 Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks 的解析）
- [x] 1.4 (紅) 寫測試 `parse_is_best_effort_on_bad_json` 與 `matcher_wildcard_and_exact`：壞結構/缺欄位回空清單不 panic；`matches("*", x)` 與 `matches("", x)` 皆真、`matches("Bash","Bash")` 真、`matches("Bash","Read")` 假
- [x] 1.5 (綠) 實作 `matches(matcher: &str, tool_name: &str) -> bool` 並讓 `parse_hooks` 對缺欄位/非預期型別略過，讓 1.4 通過

## 2. hooks_compat 讀取薄殼（best-effort I/O）— 決策六：best-effort 失敗處理；需求 Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks

- [x] 2.1 [P] (紅) 寫測試 `collect_hooks_tags_scope` 與 `collect_hooks_is_best_effort`：以臨時目錄寫使用者 `~/.claude/settings.json` 與專案 `.claude/settings.json`，斷言 `collect_hooks(project_cwd, user_home)` 回傳兩 scope 標記正確；缺檔/壞 JSON 回空、不 panic
- [x] 2.2 [P] (綠) 實作 `collect_hooks(project_cwd: &Path, user_home: &Path) -> Vec<HookEntry>`：讀兩個 settings.json、各自 `parse_hooks` 標 scope，缺檔/壞 JSON best-effort 略過，讓 2.1 通過

## 3. conn 層 tool-wrapper 執行點 — 決策一：hook 引擎放在 conn 層 tool-wrapper，不動 agent-core；決策三：hook 於 origin device 執行；決策四：PreToolUse 否決走既有 ApprovalGate 語意；需求 Run hooks around tool calls on the origin device；需求 PreToolUse hooks can deny a tool call

- [x] 3.1 在 `crates/fleety-server/src/conn.rs` 定義 hook 執行的私有函式 `run_hook`（輸入 command + origin binding + scope，回傳 exit 結果並寫 audit）：同主機用本機 shell、跨裝置經 `device_exec` 命令執行送到 origin；抽出可注入的 runner 邊界以利測試
- [x] 3.2 (紅) 寫測試 `pretooluse_nonzero_exit_denies_tool`：以假工具 + 假 hook runner（回非零 exit）包一層 wrapper，斷言工具未執行、agent 收到「被 hook 拒絕」的工具結果（比照 `ApprovalGate` 拒絕形狀）
- [x] 3.3 (綠) 在該對話工具 registry 組裝點加入 tool-wrapper：工具 `call` 前跑匹配的 PreToolUse hooks，非零 exit 即比照 `ApprovalGate` 否決、回被拒結果、不呼叫內層工具，讓 3.2 通過（實現需求 Run hooks around tool calls on the origin device 與需求 PreToolUse hooks can deny a tool call）
- [x] 3.4 (紅) 寫測試 `posttooluse_failure_does_not_block`：假工具正常回傳、匹配的 PostToolUse hook runner 失敗，斷言工具結果照常回給 agent、audit 記到警告
- [x] 3.5 (綠) 在 wrapper 內層工具回傳後跑匹配的 PostToolUse hooks，失敗只記 audit 警告、不阻斷結果，讓 3.4 通過

## 4. 安全模型：opt-out 與專案 hooks 治理 — 決策五：opt-out 安全模型與專案 hooks 供應鏈防護；需求 Hook executions are audited and project hooks are governed

- [x] 4.1 (紅) 寫測試 `project_hooks_disabled_by_env`：設 `FLEETY_DISABLE_PROJECT_HOOKS=1` 時 `collect_hooks`（或其上層彙整）不含 project scope、使用者 scope 仍在（測試用 serial + 清 env）
- [x] 4.2 (綠) 在彙整 hook 清單處讀 `FLEETY_DISABLE_PROJECT_HOOKS`，為 `1` 時濾除 project scope，讓 4.1 通過
- [x] 4.3 (紅) 寫測試 `hook_execution_is_audited_with_scope`：一次 user scope 與一次 project scope hook 執行，斷言 audit 各記一筆、project 那筆標記 `project-sourced`
- [x] 4.4 (綠) 在 `run_hook` 寫 audit：記命令、結果、scope，project scope 標 `project-sourced`，讓 4.3 通過（實現需求 Hook executions are audited and project hooks are governed）

## 5. 串接與驗證

- [x] 5.1 在 conn 對話組裝點呼叫 `hooks_compat::collect_hooks` + env 過濾，把清單交給 tool-wrapper；確認 `build_full_registry` 各呼叫點（含 subagent 路徑）不因新參數破編譯
- [x] 5.2 跑 `cargo test -p fleety-server` 全綠、`cargo clippy -p fleety-server`（`unwrap_used`/`expect_used` 無新違規）；修正殘留

## 對映 Traceability

需求 → 任務：

- 需求「Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks」→ 任務 1.1–1.5、2.1–2.2（解析 + 讀取薄殼），對應決策二「hooks_compat 為純函式解析模組」。
- 需求「Run hooks around tool calls on the origin device」→ 任務 3.1、3.3、3.5、5.1（tool-wrapper 執行點、本機/跨裝置執行），對應決策一「hook 引擎放在 conn 層 tool-wrapper，不動 agent-core」與決策三「hook 於 origin device 執行」。
- 需求「PreToolUse hooks can deny a tool call」→ 任務 3.2–3.5，對應決策四「PreToolUse 否決走既有 ApprovalGate 語意」與決策六「best-effort 失敗處理」（PostToolUse 失敗不阻斷）。
- 需求「Hook executions are audited and project hooks are governed」→ 任務 4.1–4.4，對應決策五「opt-out 安全模型與專案 hooks 供應鏈防護」。
