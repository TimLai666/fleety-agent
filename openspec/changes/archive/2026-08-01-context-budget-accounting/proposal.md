## Summary

修正 Fleety 每次模型請求都在浪費 context 的三個問題：compaction 的字元預算把不可壓縮的 system prompt 也算進去（導致壓縮幾乎每回合觸發、並吞掉本該常駐的 AGENTS.md 前言）、四個讀取工具把同一份檔案切片送兩次，以及整個 runtime 沒有任何 token 用量計量因此無從驗證優化成效。

## Motivation

研究 pi agent harness（earendil-works/pi）的設計後，比對 Fleety 現況，量到的固定開銷落差：pi 的 system prompt 加工具定義合計低於 1000 tokens，Fleety 則是 system prompt 57,135 字元加上約 100 個工具 schema 約 46 KB，換算約 30K tokens，每一次請求都重付。

但真正該先處理的不是架構移植，而是三個可獨立驗證的缺陷：

**一、compaction 的預算門檻被 system prompt 淹沒（正確性 bug，非僅效率問題）**

`agent-core` 的字元估算函式把訊息陣列的全部元素相加，其中第一個元素就是 57,135 字元的 system prompt，而預設門檻是 24,000 字元。門檻因此從第一則訊息起恆真，實際唯一生效的閘門只剩訊息則數（保留頭部一則加最近八則再加一）。結果是對話超過約十一則訊息（約兩到三輪）之後，每一回合都重建 context 並多付一次摘要模型呼叫。

更嚴重的是連帶的規格違規。伺服器每回合把訊息組成「完整 system prompt、當前時間、origin 前言、本機 AGENTS.md / CLAUDE.md、遠端 AGENTS.md」再接持久化歷史，但 compaction 只保護第一則。第二則之後的 ephemeral 前言會被摘要吞掉；又因為增量摘要的水位很快蓋過這些位置，之後每回合新產生的前言被直接丟棄且不再進入摘要，模型看到的是第一次壓縮時的過期時間戳。

這直接違反 `instruction-file-injection` 既有規格中「Scenario: injection survives compaction」所要求的行為：指令檔應該因為走 ephemeral 前言而在壓縮後仍每回合存在。該規格是對的，實作是錯的。

**二、檔案讀取工具把同一份內容送兩次**

`read_file` 同時回傳原始 `content` 與加了行號的 `numbered`，兩者是同一份切片。工具結果的字元預算是 8000 字元，重複讓模型單次實際能讀到的檔案內容直接砍半，且更容易觸發截斷。

這個重複不只一處。依專案的變更完整性規則實際稽核加行號輔助函式的全部呼叫點後，確認共有四個讀取工具屬於同一個平行介面家族且全部有相同重複：`read_file`（工作區）、`skill_read_file`（skill 內檔案）、`memory_read`（核心記憶檔）、`wiki_read`（知識維基筆記）。四者都回傳「一段切片的兩種視圖」，共用同一個 8000 字元的工具結果預算，因此必須一起改；只改其中一部分正是該規則禁止的不一致狀態。

`edit_file` 與 `skill_edit_file` 對「已變更區域」回傳的加行號視圖不屬於這個家族：那是編輯後的小範圍確認輸出，沒有對應的原始內容欄位可重複，維持不動。

現行 `filesystem-tools`、`agent-memory`、`knowledge-wiki` 規格都明文要求同時回傳兩者，所以這是規格層級的變更，不是修 bug。

**三、沒有任何 token 用量計量**

全 workspace 沒有任何地方解析供應商回傳的 usage 欄位，context 大小一律以字元估算。沒有度量就無法驗證上述兩項修正是否真的省下開銷，也無從判斷現有供應商的隱含 prefix cache 命中率。因此計量必須先落地。

## Proposed Solution

**一、預算只衡量可壓縮的部分，並保護整段前言**

將不可壓縮的開頭訊息排除在字元預算之外：預算只計算可被摘要或保留的歷史區段。同時把「保留頭部」的定義從「第一則若為 system」改為「開頭連續的所有 system 訊息」，讓當前時間、origin 前言與指令檔前言整段原樣保留，恢復 `instruction-file-injection` 規格要求的行為。

門檻值維持寫死的常數，不引入新的環境變數。

**二、檔案讀取只回一份加行號的視圖**

四個讀取工具（`read_file`、`skill_read_file`、`memory_read`、`wiki_read`）一律移除 `content`，只回傳沿用現有格式的 `numbered`，並保留 `start_line`、`end_line`、`line_count` 與各自既有的識別欄位。四者的工具說明都需明講行號前綴不屬於檔案內容，讓模型在做精確字串比對編輯時知道要剝除。

**三、加入 token 用量計量**

在模型回應型別上新增用量欄位，三個供應商各自解析原生 usage 結構（含 cached token 欄位），並在回合結果上彙總。串流路徑需要向供應商要求在最終片段附上用量。

## Non-Goals

本次不處理，明確排除：

- 工具漸進式載入（把約 100 個工具收斂成核心少數加一個工具搜尋入口）。這是本次量測要支撐的下一步，範圍大且需要獨立提案。
- system prompt 分層與章節下沉為 skill。
- Anthropic 供應商與 `cache_control` 斷點。Fleety 目前沒有 Anthropic 供應商。
- 每 token 金額換算與費用表。需要各模型定價資料，屬於另一個題目。
- 把用量寫進持久化事件流。本次只到回合結果層級，避免更動既有事件的序列化結構。
- reflection 回合、goal 自動續轉、subagent 重付 system prompt 等其他已知開銷來源。

## Alternatives Considered

**compaction 改為以 token 而非字元計量**：更準確，但需要 tokenizer 依賴且各供應商切詞不同，成本遠高於本次要解的問題。字元估算的方向性足夠，真正的錯誤是把不可壓縮的部分算進預算。

**把 ephemeral 前言改到 compaction 之後才注入**：也能解決前言被吞的問題，但要動到伺服器與 agent 核心的呼叫順序，且 `agent-core` 不應該知道呼叫端的前言語意。擴大保留頭部的定義是更小且更符合既有責任邊界的做法。

**`read_file` 改為保留 `content`、移除 `numbered`**：精確字串比對編輯最安全，但 `edit_file` 的行範圍模式會變得難用，模型得自己數行。已評估後不採用。

**`read_file` 加參數讓模型自選視圖**：schema 變大、要多教模型一個概念，且模型可能兩個都要而回到原點。已評估後不採用。

**compaction 門檻開成環境變數**：跑小 context window 本地模型的使用者可能需要調低。但依專案的變更完整性規則，新增一個 `FLEETY_` 變數必須同步更新環境變數文件、設定登錄表、說明文件與相關規格，成本高於本次收益。已評估後不採用，留待實際遇到再處理。

## Impact

- Affected specs:
  - New: `model-usage-accounting`
  - Modified: `cached-context-compaction`, `filesystem-tools`, `skills-management`, `agent-memory`, `knowledge-wiki`
  - Conformance restored (no requirement change): `instruction-file-injection`
- Affected code:
  - Modified:
    - crates/agent-core/src/agent.rs
    - crates/agent-core/src/model.rs
    - crates/agent-core/src/openai.rs
    - crates/agent-core/src/gemini.rs
    - crates/agent-core/src/codex_responses.rs
    - crates/fleety-tools/src/lib.rs
    - crates/fleety-server/src/skills.rs
    - crates/fleety-server/src/tools.rs
    - crates/fleety-server/src/wiki.rs
  - New: (none)
  - Removed: (none)
