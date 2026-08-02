## Problem

伺服器有兩條讀取遠端檔案的路徑完全不運作，且失敗被靜默吞掉：

**一、跨裝置指令檔注入**：當對話的來源是另一台裝置時，伺服器應該透過 `device_exec` 讀取該裝置 cwd 的 `AGENTS.md` / `CLAUDE.md` 並注入 context。這條路徑從未成功過。

**二、Claude hooks 探索**：伺服器應該讀取 `.claude/settings.json` 來解析 hooks 設定。這條路徑也從未成功過。

第一項直接違反 `instruction-file-injection` 既有規格的「Scenario: cross-device conversation reads the origin device's files」——規格要求跨裝置來源必須經由 `device_exec` 讀取並注入指令檔內容，實作沒有達成。

## Root Cause

兩處都用錯了參數鍵名。`read_file` 工具要求 `path`，但這兩處傳的是 `file`：

- hooks 探索傳 `{"file": <settings.json 路徑>}` 給 `read_file`
- 跨裝置注入傳 `{"device": …, "tool": "read_file", "args": {"file": <路徑>}}` 給 `device_exec`

`read_file` 以 `require_str(&args, "path")` 取參數，缺鍵時回傳錯誤。兩處呼叫都包在 `if let Ok(res)` 裡，錯誤因此被丟棄、迴圈繼續，外部觀察不到任何徵兆——沒有日誌、沒有警告，功能只是靜默地什麼都不做。

已用 `git show` 確認此問題早於 `research/pi-harness` 分支基準點（commit 70547be）就存在，不是近期迴歸。

第二層問題：兩處拿到結果後都讀 `content` 欄位，但 `read_file` 在 `context-budget-accounting` 變更後已不再回傳 `content`，改為只回傳加了行號的 `numbered` 視圖。因此就算只修鍵名，兩處仍會拿不到資料——hooks 那條會解析 JSON 失敗，指令檔那條會注入空內容。兩者必須一起修。

## Proposed Solution

**一、修正參數鍵名**：兩處都改傳 `path`。

**二、新增反向行號函式**：在 `fleety-tools` 提供一個公開函式，把加行號視圖還原成原始文字（移除每行的行號與定位字元前綴）。兩處共用同一個函式，避免各自實作而漂移。

行號格式是既有的 `{:>6}\t{內容}`，還原時以第一個定位字元為界切分；不含定位字元的行原樣保留，避免把內容本身含定位字元的情況切壞。

**三、兩處改用該函式**：hooks 探索還原後再解析 JSON；跨裝置注入還原後再送進既有的長度上限處理。

**四、補上迴歸測試**：兩條路徑各一個測試，鎖定「用錯鍵名會被抓到」與「還原後內容正確」。

## Non-Goals

- 不改 `read_file` 的回傳形狀。四個讀取工具回傳單一加行號視圖是 `context-budget-accounting` 已定案的決策。
- 不把 `read_file` 改成同時接受 `file` 與 `path`。容忍錯誤鍵名會讓同類 bug 更難被發現。
- 不處理 `fleety-cli` 那組時序不穩的握手測試，那是獨立問題。
- 不改變 hooks 設定的解析語意或指令檔的長度上限。

## Success Criteria

- 跨裝置來源的對話，其 `AGENTS.md` / `CLAUDE.md` 內容確實出現在送給模型的 context 中，且不含行號前綴。
- `.claude/settings.json` 能被成功讀取並解析出 hooks。
- 兩條路徑各有一個測試，若參數鍵名再次寫錯會失敗而非靜默通過。
- 全 workspace 建置、測試、clippy 無新增錯誤或警告。

## Impact

- Affected specs:
  - Conformance restored (no requirement change): `instruction-file-injection`
  - Modified: `filesystem-tools`
- Affected code:
  - Modified:
    - crates/fleety-tools/src/lib.rs
    - crates/fleety-server/src/conn.rs
  - New: (none)
  - Removed: (none)
