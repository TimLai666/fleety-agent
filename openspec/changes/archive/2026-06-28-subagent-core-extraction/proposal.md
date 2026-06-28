## Summary

把 subagent 的通用機制下沉到 agent-core(新 `subagent-framework` 能力),fleety-server 縮成一個 host 實作;對外行為完全不變。

## Motivation

agent-core 的鐵律是「未來可抽出去獨立的通用 agent 框架,不依賴任何 Fleety crate」。subagent 是通用 agent 框架本來就該具備的能力,但目前整包寫在 fleety-server,架構位置不對 —— 別的 embedder(CLI、其他 app)沒辦法重用。agent-core 已依賴 tokio,所以含背景任務的通用 manager 可以放核心,不增依賴。把通用機制下沉、I/O 留 host,既能讓核心自帶 subagent,又維持核心的零 Fleety 依賴。

## Proposed Solution

在 agent-core 新增 subagent 模組,提供:
- 型別 SubagentState(Spawned/Running/Done/Failed/Stopped)、SubagentMode(Spawn/Fork)、SpawnRequest。
- trait SubagentHost,把所有 I/O 抽成 embedder 實作:`resolve_provider`、`child_registry`(不含 orchestration 工具)、`initial_messages`、`gate`、`prepare_workspace`/`cleanup_workspace`、`record_events`、`on_complete`。
- SubagentManager:持 Arc<dyn SubagentHost> + 任務表 + 狀態機 + 並行上限;spawn/send/stop/status,前景 await、背景 tokio::spawn,run_turn 編排。
- register_orchestration:4 個通用 Tool(spawn_subagent / send_subagent_message / stop_subagent / subagent_status),只在頂層註冊 → 一層巢狀上限由此強制。

fleety-server 的 subagent 模組改寫為 FleetyHost(impl SubagentHost):registry 走 build_full_registry、messages/audit 走 storage、worktree 走 git、on_complete 走 conn 的 turn driver(持 turn_lock)。conn 改成建 FleetyHost → SubagentManager → register_orchestration。providers 的 ProviderTiers 維持在 server(讀 FLEETY_* 與 echo provider,屬 Fleety)。

## Non-Goals

- 不改任何對外行為:4 個工具的名稱/參數/語意、spawn/fork、tier 選擇、背景主動喚醒、worktree 隔離全部不變。
- 既有 subagent-delegation / subagent-lifecycle / economy-model-tier 三個能力的 requirements 不變,不產生 delta。
- 不把 worktree/isolation 通用化成跨 VCS 抽象;它在核心是不透明字串,git 實作留在 host。
- 不動 ProviderTiers 的位置(留 fleety-server)。
- 不新增對外設定或工具。

## Alternatives Considered

- **維持現狀(全放 fleety-server)**:最少工,但違反「通用框架自帶 subagent」與核心可抽離的目標,別的 embedder 無法重用。否決。
- **把 run_turn 留 host、核心只給狀態機**:邊界較淺,核心拿不到「跑一個子 agent」這個核心價值。改採核心呼叫 run_turn、host 只供 I/O 的 granular-callback 邊界。

## Impact

- Affected specs: new capability subagent-framework. Modified: none (subagent-delegation / subagent-lifecycle / economy-model-tier requirements unchanged).
- Affected code:
  - New: crates/agent-core/src/subagent.rs (generic mechanism: types, SubagentHost trait, SubagentManager, register_orchestration), plus its export in crates/agent-core/src/lib.rs
  - Modified: crates/fleety-server/src/subagent.rs (rewritten as the FleetyHost impl of SubagentHost), crates/fleety-server/src/conn.rs (build FleetyHost + SubagentManager + register_orchestration)
  - Removed: none
- Key acceptance: agent-core still depends on no Fleety crate; existing fleety-server subagent tests stay green; behaviour unchanged.
