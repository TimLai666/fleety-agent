## Context

subagent 目前整包在 crates/fleety-server/src/subagent.rs:SubagentRuntime 同時做了「通用機制」(任務表、狀態機、並行上限、spawn/fork/send/stop/status、4 個 Tool、run_turn 編排)與「Fleety I/O」(build_full_registry、storage、git worktree、conn 喚醒)。agent-core 是要抽出去的通用框架、零 Fleety 依賴,且已依賴 tokio。把通用機制下沉核心、I/O 留 host,核心就自帶 subagent 而仍零 Fleety 依賴。

## Goals / Non-Goals

**Goals:**

- agent-core 提供通用 subagent 機制(型別 + SubagentHost trait + SubagentManager + register_orchestration),零 Fleety 依賴。
- fleety-server 成為 SubagentHost 的一個實作,行為與現況逐位元等價。
- 一層巢狀上限、tier 選擇、背景主動喚醒、worktree 隔離全部保留。

**Non-Goals:**

- 不改對外行為或設定。
- 不把 git worktree 通用化(核心對 isolation/workspace 用不透明字串)。
- 不搬 ProviderTiers(留 fleety-server)。

## Decisions

1. **邊界 = granular callbacks,核心呼叫 run_turn。** 核心的 SubagentManager 自己跑 run_turn,只把 I/O 透過 SubagentHost 取得(provider / 子 registry / 初始 messages / gate / workspace / 稽核 / 完成回報)。這讓「跑一個子 agent」這個核心價值留在核心。

2. **SubagentHost trait(async_trait)。** 方法:resolve_provider(tier)->Arc<dyn ModelProvider>;child_registry(workspace: Option<&str>)->ToolRegistry(基礎工具,不含 orchestration);initial_messages(mode, prompt)->Vec<Message>(host 自知 system prompt 與當前對話);gate(allowed_tools)->Box<dyn ApprovalGate + Send>;prepare_workspace(isolation: &str, task_id: &str)->Result<Option<String>>;cleanup_workspace(workspace: Option<&str>)->bool(乾淨才移除,回傳是否移除);record_events(&EventLog);on_complete(task_id, state, output)(背景完成回報,host 自決如何喚醒)。

3. **SubagentManager 持 Arc<dyn SubagentHost>(trait object,不泛型)。** 持任務表 Mutex<HashMap>、running/seq AtomicU64、max_concurrent。spawn 前景 await 回 output;背景 tokio::spawn,完成後呼叫 host.on_complete。一層上限:orchestration 工具只由 register_orchestration 在頂層加,子 registry 由 host.child_registry 提供(不含)。

4. **isolation/workspace 用不透明字串。** 核心不認得 git;prepare_workspace 回一個 host 自定的 workspace 字串(Fleety 用 worktree 路徑),原樣傳回 child_registry。核心 git-agnostic。

5. **Fleety host 等價對應。** FleetyHost 把現有 SubagentRuntime 的 I/O 部分搬過來:registry=build_full_registry、provider=ProviderTiers::resolve、messages=storage.system_prompt+load、gate 同規則、worktree=git、record_events=storage.append_history、on_complete=conn::drive_turn(持 turn_lock)。turn_lock / active_conversation 留 host。

6. **ProviderTiers 留 fleety-server。** 它讀 FLEETY_* 與 EchoProvider(Fleety 專屬),不下沉。host 的 resolve_provider 內部用它。

## Implementation Contract

**Behavior:** 對外完全不變 —— spawn_subagent / send_subagent_message / stop_subagent / subagent_status 的名稱、參數、回傳、spawn/fork、main/cheap tier、背景喚醒、worktree 行為與現況等價。

**Interfaces(agent-core 新公開 API):** SubagentState、SubagentMode、SpawnRequest、trait SubagentHost、struct SubagentManager(new(host, max_concurrent) -> Arc<Self>;spawn/send/stop/status 回 serde_json::Value)、fn register_orchestration(&mut ToolRegistry, Arc<SubagentManager>)。皆 re-export 於 agent-core 的 lib。

**Failure modes:** 未知 task_id → 可行動錯誤;對執行中 send → 拒絕;超過並行上限 → 可行動錯誤;子 agent 內部錯誤 → state=Failed + 錯誤摘要當 output(永不 panic);worktree host 失敗 → host 回可行動錯誤。

**Acceptance criteria:**
- crates/agent-core 編譯後仍不依賴任何 Fleety crate(以 cargo tree / 檢視 Cargo.toml 確認無 fleety-* 依賴)。
- agent-core 新增 mock-host 單元測試:spawn 回 output、tier 路由到指定 provider、子 registry 不含 4 個 orchestration 工具而頂層含、未知 task 報錯、stop→stopped、並行上限超過報錯。
- fleety-server 既有 8 個 subagent 測試續綠(行為不變);wake 整合測試續綠。
- cargo fmt + cargo clippy --workspace -D warnings + cargo test --workspace 全綠。

**Scope boundaries:** In:agent-core 新 subagent 模組 + lib 匯出、fleety-server subagent.rs 改寫為 FleetyHost、conn 接線調整、測試遷移/新增。Out:對外行為/設定、ProviderTiers 位置、git worktree 通用化、其他 crate。
