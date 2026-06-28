<!-- 每項含交付行為 + 驗證目標。tdd:true → 核心測試先寫。boa async 已被 scratch-probe 驗證。 -->

## 1. 新 crate 與引擎

- [x] 1.1 建立 crate crates/agent-workflow(Cargo.toml 依 agent-core + boa_engine 0.21 + tokio + serde_json + async-trait;加進 root Cargo.toml workspace members)—— 交付 "The framework core stays engine-free":boa 只在這個 crate。驗證:cargo build -p agent-workflow 綠;cargo tree -p agent-core 無 boa_engine、無 fleety-*。
- [x] 1.2 實作 WorkflowRuntime 與 run_script(script) -> Result<Value>:ContextBuilder + 自留 Rc<SimpleJobExecutor>;註冊原生 agent()(from_async_fn)與 log();先 eval 一段 JS prelude 注入 parallel/pipeline/phase;eval 腳本後用 run_jobs_async 驅動到完成、取回 default/最終值 —— 交付 "Run a model-written workflow script" 與 "Deterministic control-flow primitives"。整段跑在專屬 std::thread + current-thread runtime(boa !Send),結果用 channel 橋回。驗證:單元測試跑一段含 `await agent(...)` + parallel 的腳本,斷言結果字串。

## 2. agent() 接 subagent 與 leaf 上限

- [x] 2.1 讓 agent(opts) 解析 { prompt, model?, mode?, isolation?, name?, allowed_tools? } 並呼叫共享 agent_core::SubagentManager.spawn(前景),把 output 回給 JS —— 交付 "agent() runs a leaf subagent"。驗證:用注入式 SubagentManager(mock SubagentHost,agent 回固定字串)的測試斷言 agent() 回那個字串;subagent 的 child registry(mock host 提供)不含 orchestration/workflow 工具。

## 3. 失敗處理

- [x] 3.1 實作 never-panic 失敗路徑:缺 script/meta、JS throw 未捕捉、agent step 失敗、boa thread panic 都轉成可行動錯誤回 run_script/run_workflow,主進程不倒 —— 交付 "Never-panic failure handling"。驗證:單元測試「腳本 throw 未捕捉 → run_script 回 Err 含訊息、不 panic」與「缺 meta → 回錯誤」。

## 4. 工具與接線

- [x] 4.1 在 agent-workflow 加 run_workflow 工具(參數 script、risk Mutate)與 register_workflow(&mut ToolRegistry, Arc<SubagentManager>);在 crates/fleety-server/src/conn.rs 把連線已建好的 SubagentManager 傳進去、在頂層 registry 註冊(與 register_orchestration 並列)—— 交付 run_workflow 對 agent 可見且為頂層工具。驗證:fleety-server 測試斷言頂層 registry 含 run_workflow、subagent 的 child registry 不含(維持 "agent() runs a leaf subagent" 的一層上限)。
- [x] 4.2 文件:docs/tools.md 增 run_workflow 一節;prompts/protocol.md 在編排決策裡把 pattern #4(dynamic workflow)指到 run_workflow。驗證:內容審查,工具列與設計一致。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心無引擎依賴 —— 交付 "The framework core stays engine-free" 的關鍵驗收。驗證:cargo fmt、cargo clippy --workspace -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 boa_engine 與無 fleety-*。
