## Context

我們已有 SubagentManager(spawn 一個 subagent)與 agent-team 薄層(名字定址 + roster)。缺的是「確定性編排」:模型即時寫一段腳本,把自己的 subagent 用 sequential/parallel/pipeline/phase 串起來,可重現、可版本控管。Claude Code 的 Workflow 就是這個;ODW 是同概念但編排外部 CLI。我們要內部版:`agent()` 跑自己的 subagent。

scratch-probe(boa 0.21.1)已證:boa 純 Rust、Windows MSVC 可編、JS 的 `await agent()` 能接 Rust async(tokio)、`Promise.all` 給 parallel。

## Goals / Non-Goals

**Goals:**
- 新 crate `agent-workflow` 提供一個 boa 驅動的 workflow runtime + `run_workflow` 工具。
- JS 全域 `agent()`/`parallel()`/`pipeline()`/`phase()`/`log()` + `meta`,`agent()` 跑一個 leaf subagent。
- agent-core 不碰 boa(精簡、可抽離);boa 隔離在 agent-workflow。

**Non-Goals:**
- 不連外部 CLI;不做 worker↔worker 直連;不做宣告式格式;不換 JS 引擎;不改既有 subagent/team 工具。

## Decisions

1. **新 crate `agent-workflow`(依 agent-core + boa_engine)。** boa 是重依賴,隔離在這裡;agent-core 保持精簡且零 boa。fleety-server 依 agent-workflow。

2. **boa 執行模型(已被探針接地)。** Context 用 ContextBuilder + 自留的 Rc<SimpleJobExecutor> 建;`agent()` 用 NativeFunction::from_async_fn 註冊成原生 async 函式,內部 await SubagentManager.spawn;事件迴圈用 Rc<SimpleJobExecutor>::run_jobs_async 在 runtime 上驅動。

3. **!Send 橋接。** boa Context 含 Rc/RefCell → !Send。fleety-server 是多執行緒 tokio,工具 future 要 Send。決策:run_workflow 工具的 call() 把腳本丟到一條專屬 std::thread,thread 內建 current-thread tokio runtime 跑 boa + run_jobs_async;`agent()` 在該 thread 上 await 共享的 SubagentManager(Send+Sync,跨執行緒安全);最終結果用 std oneshot/channel 橋回 call(),call() 以 spawn_blocking 或 channel await 等待。

4. **agent() 語意。** 解析 opts({ prompt, model?, mode?, isolation?, name?, allowed_tools? }),呼叫 SubagentManager.spawn(前景),回傳 subagent 的 output 字串給 JS。背景模式在 workflow 內不開放(workflow 本身就是編排,要結果);schema 結構化輸出列為後續增強。

5. **parallel/pipeline/phase/log。** Promise.all 已給併發;parallel(thunks 陣列)= 包成 Promise.all;pipeline(items, ...stages)= 對每個 item 串過各 stage(JS helper 注入);phase(name)/log(msg)= 進度標記,透過 host 回呼寫進記錄/串流。這些用注入的 JS prelude(一段 boa 先 eval 的 JS)實作,核心只需 agent()+log() 兩個原生函式 + Promise.all。

6. **一層巢狀上限維持。** workflow 的 agent() 跑 leaf subagent:subagent 的 child registry 不含 orchestration 也不含 run_workflow,所以 subagent 不能再開 workflow 或 subagent。

7. **失敗永不 panic。** 腳本語法/執行錯誤 → 可行動錯誤字串;meta 缺失 → 拒絕;agent 步驟失敗 → 該 agent() reject,腳本可自行 catch,未捕捉則整個 run_workflow 回錯誤。逾時/步數上限沿用 SubagentManager 與 LoopConfig。

## Implementation Contract

**Behavior:** agent 呼叫 `run_workflow({ script })`;腳本同步寫成 `meta` + 用 `await agent(...)` / `parallel` / `pipeline` / `phase` / `log`;runtime 跑完回傳腳本的 `export default`/最終值(序列化成 JSON)。`agent()` 每次跑一個 Fleety subagent 並回其 output。

**Interfaces:**
- 工具 `run_workflow`,參數 `script`(string,必填),risk Mutate;回傳 { result: <腳本結果>, phases?: [...], logs?: [...] }。
- crate agent-workflow 公開:`WorkflowRuntime`(持 Arc<SubagentManager> + 一個 host 回呼介面寫 log/phase)、`register_workflow(&mut ToolRegistry, Arc<SubagentManager>, ...)`、以及 `run_script(script) -> Result<Value>` 供測試直接呼叫。
- JS API(注入):`meta = {name, phases?}`;`agent({prompt, model?, mode?, isolation?, name?, allowed_tools?}) -> Promise<string>`;`parallel(thunks) -> Promise<any[]>`;`pipeline(items, ...stages) -> Promise<any[]>`;`phase(name)`;`log(msg)`。

**Failure modes:** 無 script → 可行動錯誤;JS parse/throw 未捕捉 → 回錯誤含訊息;agent step 失敗 → 該步 reject;boa thread panic 被捕捉成錯誤(run_workflow 不讓 server crash);任何情況都不 panic 主進程。

**Acceptance criteria:**
- crates/agent-core 仍不依賴 boa、不依賴任何 fleety crate(以 cargo tree 確認)。
- agent-workflow 單元測試:用注入式 SubagentManager(mock SubagentHost,agent() 回固定字串)跑一段 `const a=await agent({prompt:"x"}); const ps=await parallel([()=>agent({prompt:"p1"}),()=>agent({prompt:"p2"})]); export default a+"|"+ps.join(",")`,斷言結果字串。
- 另一測試:腳本 throw 未捕捉 → run_script 回 Err 且含訊息(不 panic)。
- fleety-server:run_workflow 在頂層 registry,subagent 的 child registry 不含它(斷言)。
- cargo fmt + clippy --workspace -D warnings + test --workspace 全綠。

**Scope boundaries:** In:新 crate agent-workflow(boa runtime + JS prelude + run_workflow 工具)、root Cargo.toml 加 member、fleety-server conn 註冊、docs/tools.md 與 prompts/protocol.md、測試。Out:外部 CLI、worker↔worker、宣告式格式、schema 結構化輸出(列後續)、背景 workflow、agent-core 任何 boa 依賴。
