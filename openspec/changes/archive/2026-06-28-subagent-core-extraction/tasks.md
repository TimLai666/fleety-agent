<!-- 純內部重構,行為不變。每項含交付行為 + 驗證目標。tdd:true → 核心新測試先寫。 -->

## 1. agent-core 通用機制

- [x] 1.1 在 crates/agent-core/src/subagent.rs 建立通用機制 —— 交付 "Generic subagent mechanism in the core" 與 "Host trait abstracts all I/O":SubagentState/SubagentMode/SpawnRequest、trait SubagentHost(resolve_provider/child_registry/initial_messages/gate/prepare_workspace/cleanup_workspace/record_events/on_complete)、SubagentManager(任務表+狀態機+並行上限+seq)、register_orchestration(4 個通用 Tool),全部不依賴任何 Fleety 型別,run_turn 編排在核心。驗證:crates/agent-core 編譯通過且 Cargo.toml/cargo tree 無 fleety-* 依賴。
- [x] 1.2 在 crates/agent-core/src/lib.rs 匯出新公開 API(SubagentState/SubagentMode/SpawnRequest/SubagentHost/SubagentManager/register_orchestration)—— 交付 "Manager owns the lifecycle" 的可取用介面。驗證:agent-core 對外可 use 這些型別,cargo build -p agent-core 綠。

## 2. agent-core 測試(mock host)

- [x] 2.1 在 agent-core subagent 模組加一組以 mock SubagentHost 驅動的單元測試 —— 交付 "One-level nesting cap by construction" 與 "Manager owns the lifecycle" 的驗收:子 registry 不含 4 個 orchestration 工具而頂層含、前景 spawn 回 output、tier 路由到指定 provider、未知 task 報錯、stop→stopped、超過並行上限報錯。驗證:cargo test -p agent-core subagent:: 全綠。

## 3. fleety-server host 改寫

- [x] 3.1 把 crates/fleety-server/src/subagent.rs 改寫成 FleetyHost(impl agent_core::SubagentHost)—— 交付 "Host trait abstracts all I/O" 的 Fleety 實作:resolve_provider 走 ProviderTiers、child_registry 走 build_full_registry、initial_messages/record_events 走 storage、gate 同規則、prepare_workspace/cleanup_workspace 走 git worktree(乾淨才移除、髒則保留回報)、on_complete 走 conn 的 turn driver 並持 turn_lock。通用機制(狀態機/任務表/4 工具)不再留在 server。驗證:行為與現況等價,既有 8 個 fleety-server subagent 測試(含 wake 整合測試)續綠。
- [x] 3.2 conn 接線改為建 FleetyHost → agent_core::SubagentManager::new → register_orchestration(頂層 registry);turn_lock / active_conversation 由 host 持有,recover 與主 turn 仍在鎖內。驗證:cargo test -p fleety-server 全綠(82+ 既有測試不破)。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心保持零 Fleety 依賴 —— 交付 "Generic subagent mechanism in the core" 的關鍵驗收。驗證:cargo fmt、cargo clippy --workspace -D warnings、cargo test --workspace 全綠;git diff 確認 crates/agent-core/Cargo.toml 未新增 fleety-* 依賴。
