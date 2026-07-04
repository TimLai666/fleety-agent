## Context

`hooks-compat` 在 conn 的對話綁定點收集 origin 的 Pre/PostToolUse hooks，用 `wrap_tools` 包裝主對話 registry。subagent 走另一條路:`FleetyHost::registry_at` 直接呼叫 `build_full_registry`，產出未包 hooks 的 registry，供 `child_registry`（spawn/fork 的子 agent）與 `on_complete` 喚醒回合使用。因此委派給 subagent 的工具呼叫不經任何 hook。

`FleetyHost` 已持有 hook 執行所需的 I/O 依賴（`storage`、`device_id`、`hub`、`pending`）與 `active_conversation`。缺的是「這條對話收集到的 hook 清單 + origin 位置」，而它在 `FleetyHost::new` 時尚未收集（收集發生在 stack build 之後、綁定點的 async 段）。

## Goals / Non-Goals

**Goals:**

- subagent（含巢狀）的工具 registry 套用主對話同一批 hooks，語意與主對話一致（PreToolUse 非零否決、PostToolUse 不阻斷、全程 audit、專案 hooks 受 env 政策）。
- 抽出單一包裝點供主對話與 subagent 共用，避免邏輯分叉。
- 不改任何既有 hook 語意。

**Non-Goals:**

- 新增 hook 事件（後續 change B）。
- 啟動時 crash 復原（無 origin，無 hooks）。
- 改動 subagent 委派機制。

## Decisions

### 決策一:抽出 `HookContext` 與 `wrap_registry_with_hooks`

在 conn 定義 `pub(crate) struct HookContext { hooks: Vec<HookEntry>, device: Option<String>, cwd: Option<String> }` 與 `pub(crate) fn wrap_registry_with_hooks(tools: &mut ToolRegistry, ctx: &HookContext, hub, pending, storage, device_id, conversation)`。後者:hooks 空則直接 return;否則以 `OriginHookRunner`（device/cwd/hub/pending）與 `HistoryHookAudit`（storage/device_id/conversation）建 runner/audit，`wrap_tools(tools.drain(), …)` 後 re-register。主對話綁定點改用此函式（取代現有 inline 迴圈），subagent 路徑亦呼叫之。

### 決策二:以設定式 handle 把 HookContext 注入 FleetyHost

`FleetyHost` 加 `hook_ctx: OnceLock<Arc<HookContext>>` 與 `set_hook_context(&self, ctx: Arc<HookContext>)`。綁定點收集完 hooks、包好主對話 registry 後，若 hooks 非空，`Arc::new(HookContext{…})` 一份，主對話包裝與 `subagent_host.set_hook_context(...)` 共用同一 Arc。`OnceLock` 於該次綁定設一次;hooks 空則不設，subagent 路徑照舊無包裝（零額外成本）。

### 決策三:在 async 呼叫點包裝，audit 掛對的 conversation

`registry_at` 為 sync 且不知 conversation，故不在其內包裝。改在 async 的 `child_registry`（讀 `active_conversation` 取 conversation）與 `on_complete`（已有 `context` 即父對話）內，建好 base registry 後呼叫 `wrap_registry_with_hooks`。`on_complete` 在 `register_orchestration` 之後包裝，故 orchestration 工具也一併納入 hook 比對（matcher 命名比對，spawn_subagent 之類不會誤中 Bash matcher;`*` matcher 一律納入屬預期語意）。

### 決策四:巢狀 subagent 自動覆蓋

subagent 再 spawn 時共用同一 `FleetyHost`（同一 `hook_ctx`），故子 registry 一律經同一包裝點，遞迴覆蓋，無需額外處理。

## Implementation Contract

**Behavior（可觀察）:**

- origin 設 PreToolUse matcher 命中某工具、command 非零 exit;agent 委派該工具給 subagent 執行時，subagent 端該工具被否決、不執行，audit 出現該 hook 執行紀錄（掛在 subagent/父對話）。
- PostToolUse hook 在 subagent 工具回傳後執行、失敗只記 audit、不阻斷。
- 主對話與 subagent 的 hook 語意一致;env 政策（`FLEETY_DISABLE_PROJECT_HOOKS`）同時影響兩者（因用同一批已過濾 hooks）。
- hooks 為空時 subagent registry 不被包裝，行為與現況相同。

**介面:**

- conn:`pub(crate) struct HookContext`;`pub(crate) fn wrap_registry_with_hooks(...)`;綁定點 inline 包裝改呼叫之並 `subagent_host.set_hook_context(...)`。
- subagent:`FleetyHost` 增 `hook_ctx: OnceLock<Arc<crate::conn::HookContext>>` 與 `set_hook_context`;`child_registry` 與 `on_complete` 於 base registry 後呼叫 `wrap_registry_with_hooks`。

**驗證目標（測試名／可觀察行為）:**

- `wrap_registry_with_hooks_denies_on_nonzero_pre`（conn）:以 device=None + 真實本機 shell command（跨平台 `exit 1`）包一個假工具，斷言呼叫被否決、audit（真實臨時 Storage）有紀錄;另一個 `exit 0` 的 case 放行。
- `empty_hook_context_leaves_registry_unwrapped`（conn）:空 hooks → 包裝為 no-op，工具正常執行。

**In scope:** conn 抽出共用包裝點、FleetyHost 注入與包裝、巢狀覆蓋。
**Out of scope:** 新 hook 事件、啟動 crash 復原、委派機制本身。

## Open Questions

- subagent hook 執行的 audit 掛在父對話或子（child）對話:首版 `child_registry` 掛 `active_conversation`（父）、`on_complete` 掛 `context`（父）。若日後要讓 subagent hook 紀錄可依 child 對話檢索，再改掛 `subagent_child_id`;首版以父對話為準，簡單且足以稽核。
