## Why

Fleety 目前是單一 agent 的 tool-loop(agent-core 的 run_turn / run_turn_streaming)。長研究、大量獨立實作、可平行的子任務都得擠在同一條 context 裡,既塞爆上下文、又無法平行、也無法用比較便宜的模型分攤雜活。導入 subagent 委派,讓主 agent 能把單一任務派給子 agent(可選便宜模型、可背景跑、可平行),自己保持輕量並綜整結果。run_turn 是純函式、吃 provider+tools+messages,本來就可巢狀,落地門檻低。

## What Changes

- 新增 subagent 委派:主 agent 可派生子 agent,子 agent 在自己的 context 內推理與用工具,完成後把結果回傳。
- 兩種模式:**spawn**(全新乾淨 context + briefing)與 **fork**(繼承父 agent messages)。兩者皆可選模型 tier;**fork 亦可換 tier**。
- 模型 tier 選擇(只用 tier,不接受任意 model 名稱):`main` 或 `cheap`,由 agent 每次派工時自選。
- 新增可選的**便宜模型**第二 provider(`FLEETY_CHEAP_MODEL_*`),與主模型可不同 provider/model;未設時 `cheap` 解析回主 provider。
- 全套非同步編排:背景執行、完成主動回報(去重)、對同一子 agent 續談、停止;含任務註冊表與狀態機。
- 子 agent 能力 = 主 agent 全集**減去** orchestration 工具(spawn/send/stop)→ 一層巢狀上限。子 agent 仍可 device_exec 操作別台裝置、用 browser/computer-use/mcp/wiki/filesystem 全部工具。
- 隔離:`none` 或 `worktree`(平行改檔的子 agent 各自獨立 git worktree)。

## Non-Goals

- **不做跨裝置 spawn(remote isolation)**:子 agent 進程本身只在 server 同進程跑;要在別台裝置做事透過既有 device_exec,不另外把「子 agent 執行器」外派。
- **不允許多層巢狀**:子 agent 永遠拿不到 orchestration 工具,無法再生子 agent。
- 不接受任意 model 名稱覆寫(只 main/cheap 兩 tier)。
- 不改 model-provider 既有規格的主模型行為;便宜模型是**另加**的第二 tier。
- 不改任何既有工具的行為;orchestration 工具是新增。
- 不做 GUI 任務監控面板、自動 agent routing、跨機分散式排程。

## Capabilities

### New Capabilities

- `subagent-delegation`: 委派機制本體 —— spawn/fork 兩模式、模型 tier 選擇(main/cheap)、能力繼承(主 agent 全集減 orchestration)與一層巢狀上限、worktree/none 隔離、子 agent 結果回傳父 agent。
- `subagent-lifecycle`: 非同步生命週期 —— 任務註冊表與狀態機(spawned/running/done/failed/stopped)、背景執行、完成主動回報與去重、對同一子 agent 續談(保留 messages)、停止子 agent。
- `economy-model-tier`: 便宜模型 tier —— 第二個獨立 provider 的設定(`FLEETY_CHEAP_MODEL_BASE_URL`/`_MODEL`/`_KEY`/`_STREAM`)、tier 解析規則、未設時回退主 provider。

### Modified Capabilities

(none)

## Impact

- Affected specs: 3 new capability specs (subagent-delegation, subagent-lifecycle, economy-model-tier); references model-provider and device-registry-and-routing without changing them.
- Affected code:
  - New: crates/fleety-server/src/subagent.rs (orchestration tools + task registry + runtime), plus any neutral nestable-runner helper in crates/agent-core/src
  - Modified: crates/fleety-server/src/main.rs (build the cheap provider + subagent runtime), crates/fleety-server/src/conn.rs (register orchestration tools, surface completion notifications to the user), docs/env.md (cheap-model variables), docs/tools.md (new orchestration tools), prompts/protocol.md (when to delegate)
  - Removed: none
