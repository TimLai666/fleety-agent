## Why

hooks-compat 目前只做工具層事件（PreToolUse／PostToolUse）。Claude Code 使用者常設的另外三個生命週期 hook——UserPromptSubmit（送出提示時）、Stop（主 agent 回應結束時）、SubagentStop（subagent 結束時）——在 Fleety 對話中完全不生效。補齊這三個事件，讓發起端的提示前守門、回合結束通知/清理、subagent 結束勾點都能沿用。

## What Changes

- `HookEvent` 擴增 UserPromptSubmit／Stop／SubagentStop 三個事件;`parse_hooks` 一併解析（沿用既有 best-effort 與 `hooks[].command` 形狀，這三者非工具層、matcher 對它們無意義故忽略）。
- 新增 `run_event_hooks(event, …)`:對匹配事件的 hooks 逐一執行＋audit;**只有 UserPromptSubmit 在非零 exit 時回報「阻擋」**（提示前守門，比照 PreToolUse 否決的安全語意），Stop／SubagentStop 一律放行（僅執行＋audit）。
- conn 服務迴圈:處理使用者訊息時，dispatch 前跑 UserPromptSubmit——非零即阻擋該提示（回一則「被 hook 阻擋」訊息、不處理該回合）;該使用者訊息完整處理完後跑 Stop。
- subagent:`FleetyHost::on_complete`（subagent 結束）跑 SubagentStop。
- 沿用既有 runner（origin 本機／跨裝置）、audit、env 政策、`HookContext`;事件 hooks 不進 tool-wrapper（`wrap_tools` 仍只包 Pre/PostToolUse）。

## Non-Goals

- **不做 Stop／SubagentStop 的「阻擋停止／強制續跑」語意**（需重入回合迴圈，屬後續）;首版只執行＋audit。
- 不做 hook stdout 注入 context（runner 目前丟棄輸出，屬後續）。
- 不做 SessionStart／PreCompact／Notification 等其他事件。
- 不改 Pre/PostToolUse 既有語意。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `hooks-compat`: hook 事件從 Pre/PostToolUse 擴到含 UserPromptSubmit（可阻擋提示）／Stop／SubagentStop（執行＋audit）。

## Impact

- Affected specs: `hooks-compat`（新增一條 requirement:生命週期事件 hooks）。
- Affected code:
  - Modified: `crates/fleety-server/src/hooks_compat.rs`（HookEvent 擴增、parse_hooks、run_event_hooks）、`crates/fleety-server/src/conn.rs`（服務迴圈 UserPromptSubmit 阻擋／Stop、事件 hook helper、保留 HookContext）、`crates/fleety-server/src/subagent.rs`（on_complete SubagentStop）
  - New: (none)
  - Removed: (none)
