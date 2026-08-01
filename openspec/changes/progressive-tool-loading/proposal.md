## Summary

把伺服器每次請求都送出的 80 個工具 schema，收斂成一組常駐核心工具加上一個工具搜尋入口，其餘依模型當下需要才啟用，讓不使用的能力不再佔用 context。

## Motivation

`context-budget-accounting` 變更加入的量測，跑出了實際數字（`measure_harness_footprint`，本機完整 registry）：

| 項目 | 實測 |
| --- | --- |
| system prompt | 59,062 字元 / 72,332 bytes |
| 工具 schema | 80 個工具 / 36,927 bytes |
| 合計 | 109,259 bytes ≈ 27,300 tokens |
| 佔 200k context | 約 13.7% |

工具 schema 是其中可以立刻收斂的一半。80 個工具裡，任何一次對話實際會用到的通常不到十個，但全部 schema 每一輪都重送，且每個 subagent 會再付一次。

研究 pi agent harness 時確認了兩件事：其一，pi 的 system prompt 加工具定義合計低於 1000 tokens，靠的正是「工具只留四個，其餘下放」；其二，pi 有一個現成的延後載入範例（`kimi-deferred-tools`），開場只註冊一個 `tool_search`，模型需要時才把工具啟用進來。

Fleety 已經有兩個同型的成功先例，證明這個模式在本專案可行且不需要新概念：裝置端的整組工具收在單一 `device_exec` 之後、外部 MCP 伺服器的工具收在單一 `mcp_call` 之後，兩者都不展開子工具 schema。skills 也早就是漸進式揭露——system prompt 不含任何 skill 內容，靠 `list_skills` 與 `use_skill` 按需載入。本變更把同一個原則套用到內建工具本身。

工具分組後的實測分佈（bytes 為含 JSON 外殼的估算 wire size）：

| 群組 | 工具數 | bytes |
| --- | --- | --- |
| skills | 10 | 5,294 |
| fleet | 12 | 4,736 |
| core-files | 10 | 4,645 |
| web | 4 | 4,545 |
| data | 2 | 2,470 |
| memory | 4 | 2,338 |
| terminal | 4 | 2,027 |
| computer | 6 | 2,008 |
| wiki | 5 | 1,824 |
| browser | 5 | 1,699 |
| sites | 5 | 1,247 |
| mcp | 4 | 1,155 |
| schedule | 3 | 1,108 |
| bytes | 2 | 1,005 |
| git | 4 | 826 |

分組涵蓋全部 80 個工具，無遺漏。

## Proposed Solution

**一、定義常駐集合**：檔案與命令核心（`read_file`、`edit_file`、`write_file`、`search_files`、`list_dir`、`make_dir`、`move_file`、`delete_file`、`rollback`、`run_command`）、skills 的兩個入口（`list_skills`、`use_skill`）、核心記憶讀寫（`memory_read`、`memory_write`）、以及跨裝置的兩個入口（`device_list`、`device_exec`）。共 16 個工具、7,146 bytes。

**二、其餘工具依群組延後**：64 個工具、29,781 bytes 不再於開場送出。

**三、新增一個工具搜尋入口**：模型以能力描述查詢，取得符合的群組與其中工具的名稱與一行摘要，並將該群組啟用進當前對話的工具集。啟用後該群組的完整 schema 才進入後續請求。

**四、啟用狀態存活於整段對話**：一旦啟用就持續有效，避免模型在同一段工作中反覆搜尋同一個群組。啟用集合隨對話持久化，重啟後仍在。

**五、system prompt 教會這個機制**：`protocol.md` 需說明「目前看得到的工具不是全部，需要別的能力時先搜尋」。這段文字本身也是成本，必須寫得極短。

## Non-Goals

- 不改 system prompt 的四份文件內容或其分層方式。那是另一個題目，且收益與風險都與本變更不同。
- 不移除任何工具。所有能力都保留，只改變它們何時進入 context。
- 不改動 `device_exec` 與 `mcp_call` 既有的代理模式，兩者已經是正確做法。
- 不引入 token 計費或成本換算。
- 不改變工具本身的行為、參數或風險分級。
- 不處理 subagent 是否應共用父代的啟用集合以外的 subagent 議題。

## Alternatives Considered

**依對話情境自動預測要載入哪些群組**：省去模型一次搜尋往返，但預測錯誤時模型會看不到自己需要的能力，且錯誤難以觀察。工具搜尋是模型自己發起的，意圖明確、可稽核。已評估後不採用。

**把工具描述整體壓短**：能省一部分，但 80 個工具的固定成本仍在，且壓縮描述會直接傷害模型選對工具的能力。與本變更不衝突，可日後另案處理。

**照 pi 只保留四個工具**：pi 是單機 coding agent，Fleety 是跨裝置 fleet 助理，`device_exec`、`browser_*`、`schedule_*` 這些是產品定義本身，砍掉等於砍功能。要移植的是投放方式，不是數量。已評估後不採用。

**讓模型逐一啟用單個工具而非整個群組**：更精準，但同一件工作通常需要群組內數個工具，逐一啟用會產生多次往返。群組是較合適的粒度。

## Impact

- Affected specs:
  - New: `progressive-tool-loading`
  - Modified: (none — no existing spec governs which tools are shown to the model)
- Affected code:
  - Modified:
    - crates/agent-core/src/tools.rs
    - crates/agent-core/src/agent.rs
    - crates/fleety-server/src/conn.rs
    - crates/fleety-server/src/tools.rs
    - crates/fleety-server/src/storage.rs
    - prompts/protocol.md
  - New: (none)
  - Removed: (none)
