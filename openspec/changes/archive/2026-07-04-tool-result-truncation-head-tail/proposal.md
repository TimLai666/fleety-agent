## Why

工具輸出截斷框架整體良好(小結果零損耗 byte-for-byte、完整結果可逆存 event log、只在真截時掛 fetch id、長陣列保留頭尾),但有兩個確認的缺陷,是稽核修 retrievable-tool-results 選項 2 時未涵蓋的:

- **A(長字串只留頭、丟尾)**:SmartCrusher 對超過 max_string 的長字串只保留開頭(取前 max_string 字元 + 「…(+N chars)」),尾端整段丟棄。而 run_command 的 stdout / stderr 是**單一字串**,所以一個長輸出只看得到開頭,尾端(常是錯誤結論、失敗 summary、exit 前的關鍵訊息)被截掉。長陣列有頭+尾邏輯,長字串卻沒有,不一致。
- **B(fetch 遞迴截斷)**:`fetch_tool_result` 本身是工具,它的結果也會回到 run_turn 再經一次 compress_tool_result。fetch 一頁的 content 最多 limit(預設=budget=8000)字元,但 SmartCrusher 的 max_string(4000)會把這個 content 字串**再砍半**並掛一個指向 fetch 自身呼叫的新 marker。根因是 budget(8000)與 max_string(4000)不一致,導致「查完整輸出」時每頁名義 8000、模型實得 4000,且冒出無意義的自我指向 marker。

## What Changes

- **A**:SmartCrusher 結構壓縮長字串時保留**頭與尾**(例如頭佔約 3/4、尾佔約 1/4,中間放「…(+N chars)」省略標記),對齊長陣列頭 20 + 尾 5 的精神,使單一字串輸出的尾端結論可見。
- **B**:讓 `fetch_tool_result` 的一頁窗口上限對齊「結構壓縮不會再縮減的大小」——即 SmartCrusher 的長字串門檻(max_string)。這樣一頁 content 不超過該門檻,回到壓縮時就不會被 SmartCrusher 二次截,也不會冒出指向自身的 marker,一頁 content 原樣抵達模型。分頁介面(offset / next_offset)形狀不變,只是每頁上限從 budget 對齊到字串門檻(較小,但保證整頁完整)。
- 兩者各附回歸測試。

## Non-Goals (optional)

(詳見 design.md 的 Goals / Non-Goals;關鍵排除:不改 budget 8000 與門檻數值本身、不改 event log 可逆機制、不改 fetch 分頁介面 offset/limit/next_offset 的形狀、不動 CodeCompressor / CacheAligner。)

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `retrievable-tool-results`: 新增「長字串結構壓縮保留頭與尾」的 requirement;修改「fetch 分段」requirement,使 fetch 回傳的內容窗口不被結構化字串截斷二次縮減。

## Impact

- Affected specs: retrievable-tool-results
- Affected code:
  - Modified:
    - crates/agent-core/src/compress.rs — SmartCrusher 長字串改頭+尾保留;暴露長字串門檻(max_string)供 fetch 對齊
    - crates/fleety-server/src/tools.rs — fetch_tool_result 一頁 limit 的上界對齊 SmartCrusher 長字串門檻,使窗口不被二次結構壓縮
  - New: (none)
  - Removed: (none)
