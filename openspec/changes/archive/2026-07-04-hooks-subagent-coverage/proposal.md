## Why

hooks-compat 首版只在**主對話**的工具 registry 上套 Pre/PostToolUse hooks。subagent 另建自己的 registry（`FleetyHost::registry_at` → `build_full_registry`），完全沒套 hooks。這是安全一致性的破口:使用者設的防護 hook（例如 PreToolUse 擋 `rm -rf`）只要 agent 把該工具呼叫**委派給 subagent** 就被繞過。安全保證不該因委派而失效。

## What Changes

- 把主對話已收集、已套用 env 政策的 hook 清單，一併套到 subagent 的工具 registry:`FleetyHost` 建 child registry 與 on_complete 喚醒回合的 registry 時，用同一批 hooks 包裝。
- 遞迴覆蓋:subagent 再 spawn 的巢狀 subagent 共用同一 host，故自動沿用同批 hooks。
- 重構出共用包裝點:conn 抽出 `HookContext`（hooks + origin device/cwd）與 `wrap_registry_with_hooks(...)`，主對話綁定點與 subagent 路徑共用同一段包裝邏輯（DRY）。
- 綁定點把 `HookContext` 交給 `FleetyHost`（建立時 hooks 尚未收集，故以設定式 handle 於收集後注入）。
- 澄清 recovery 範圍:連線內 `recover_incomplete_turn` 用的就是已包 hook 的主 registry（已涵蓋）;啟動時 crash 復原無 origin、收不到 hooks（本就 N/A）。故本次不動 recovery。

## Non-Goals

- 不新增 hook 事件（UserPromptSubmit／Stop／SubagentStop 屬後續 change B）。
- 不改 hook 執行語意、否決語意、audit 形狀、env 政策（沿用 hooks-compat 既有）。
- 不處理啟動時 crash 復原（無 origin，無 hooks 可套）。
- 不改 subagent 委派機制本身。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `hooks-compat`: hooks 的套用範圍從「主對話 registry」擴到「subagent（含巢狀）的工具 registry」，使安全 hook 無法藉委派繞過。

## Impact

- Affected specs: `hooks-compat`（新增一條 requirement:hooks 套用到 subagent 工具呼叫）。
- Affected code:
  - Modified: `crates/fleety-server/src/conn.rs`（抽出 `HookContext` 與 `wrap_registry_with_hooks`;綁定點改用之並把 context 交給 subagent host）、`crates/fleety-server/src/subagent.rs`（`FleetyHost` 收 hook context;child registry 與 on_complete registry 包裝）
  - New: (none)
  - Removed: (none)
