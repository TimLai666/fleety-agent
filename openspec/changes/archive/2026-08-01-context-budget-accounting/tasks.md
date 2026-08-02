## 1. compaction 預算語意與前言保留

- [x] 1.1 在 crates/agent-core/src/agent.rs 的測試模組新增一個失敗測試，鎖定「The compaction budget measures only compactable history」：組一段開頭前言遠超門檻、但其後歷史遠低於門檻的 context，斷言壓縮不執行、context 原樣送出、且 mock 供應商沒有收到任何摘要請求。驗證：該測試在實作前失敗、在 1.3 完成後通過。
- [x] 1.2 在同一測試模組新增一個失敗測試，鎖定「The whole leading preamble survives compaction」：組一段開頭有五則連續 system 訊息（system prompt、當前時間、origin、本機指令檔、遠端指令檔）且其後歷史超過門檻的 context，斷言壓縮後這五則原樣存在於重建結果的最前面，且其文字沒有出現在被摘要的內容中。驗證：該測試在實作前失敗、在 1.4 完成後通過。
- [x] 1.3 依設計決策「預算只計算可壓縮區段」修改字元估算與壓縮進入條件，使預算只衡量開頭不可壓縮前言之後的歷史；門檻常數維持現值不變，並在常數處加註其語意為「可壓縮歷史的預算」。觀察行為：可壓縮歷史小於門檻時不再產生摘要模型呼叫。驗證：1.1 通過。
- [x] 1.4 依設計決策「保留頭部擴大為開頭連續 system 訊息」把保留頭部的判定改為開頭最長的連續 system 訊息串，並在判定處註解說明「持久化歷史不應以 system 訊息開頭」這個前提。觀察行為：每回合重新注入的當前時間與指令檔前言原樣送達模型，不再被折進摘要。驗證：1.2 通過，且 crates/agent-core/src/agent.rs 既有的五個 compaction 測試（summarizes_old_messages、does_not_orphan_tool_messages、keeps_recent_when_tail_is_all_tools、is_incremental_with_cache、recomputes_on_config_change）全數通過；若需調整，只調整測試佈置而非放寬斷言。

## 2. 檔案讀取回傳單一視圖

- [x] 2.1 [P] 依設計決策「檔案讀取只回加行號視圖」實作 read_file 的新回傳形狀，鎖定「Read and inspect workspace files」：回傳路徑、加行號視圖、start_line、end_line、line_count，移除原始內容欄位，並更新工具說明敘明行號前綴不屬於檔案內容。觀察行為：同一次讀取送進 context 的位元組量減半。驗證：調整 crates/fleety-tools/src/lib.rs 既有的 read_numbered_and_edit_by_line 測試，斷言結果不含原始內容欄位、含加行號視圖，且行範圍切片與行號起算結果不變。
- [x] 2.2 [P] 讓 skill_read_file 對齊同一形狀，鎖定「Skill file reads return a single line-numbered view」：回傳 skill 名稱、來源層級、檔名、加行號視圖與起訖行與總行數，移除原始內容欄位，工具說明同步敘明行號前綴不屬於內容。驗證：新增一個測試，讀取一個 skill 內檔案並斷言回傳鍵集合不含原始內容欄位、且指定起訖行時切片正確。
- [x] 2.3 依 AGENTS.md 的變更完整性規則，稽核全 workspace 加行號輔助函式的每個呼叫點，界定「回傳一段檔案切片的兩種視圖」這個平行介面家族的完整成員，並記錄每個呼叫點是納入或豁免及其理由。觀察行為：家族成員在本次全部改為單一視圖，非成員（編輯後確認用的已變更區域視圖、全文載入型讀取）維持不動且有明確豁免理由。驗證：以 ripgrep 列出全部呼叫點，逐一分類並在本任務下記錄結論；結論須與 design.md 決策「檔案讀取只回加行號視圖」所列的家族成員一致。
- [x] 2.4 讓 memory_read 對齊同一形狀，鎖定「Read and edit agent core memory files」：回傳檔名、加行號視圖與起訖行與總行數，移除原始內容欄位，工具說明敘明行號前綴不屬於內容；memory_edit 的 applied 確認區域維持原狀。驗證：新增測試斷言 memory_read 回傳不含原始內容欄位、指定起訖行時切片正確，且 crates/fleety-server/src/tools.rs 既有的 memory 相關測試全數通過。
- [x] 2.5 讓 wiki_read 對齊同一形狀，鎖定「Read and write the knowledge wiki」：回傳路徑、加行號視圖與起訖行與總行數，移除原始內容欄位，工具說明敘明行號前綴不屬於內容。驗證：新增測試斷言 wiki_read 回傳不含原始內容欄位、指定起訖行時切片正確，且 crates/fleety-server/src/wiki.rs 既有測試全數通過。

## 3. token 用量計量

- [x] 3.1 依設計決策「用量型別放在模型回應與回合結果」新增用量型別並掛上模型回應，鎖定「Model responses carry provider-reported token usage」：欄位涵蓋輸入、輸出、總計與快取命中輸入數，整體為選填，供應商未回報時為空而非零。觀察行為：mock 與 echo 供應商無需改動即可繼續運作。驗證：新增單元測試斷言未回報用量的回應其用量為空，且不等同於零值用量。
- [x] 3.2 [P] 讓 OpenAI 相容供應商解析其原生 usage 物件（prompt / completion / total，以及 prompt token 明細中的 cached 欄位）並填入模型回應。驗證：新增反序列化測試，分別以帶 usage 與不帶 usage 的回應本體斷言解析結果。
- [x] 3.3 [P] 讓 Gemini 供應商解析其 usageMetadata（prompt token count、candidates token count、cached content token count）並填入模型回應。驗證：新增反序列化測試，涵蓋帶與不帶 usageMetadata 兩種回應。
- [x] 3.4 [P] 讓 Codex Responses 供應商解析其 usage 物件（input / output tokens，以及 input token 明細中的 cached 欄位）並填入模型回應。驗證：新增反序列化測試，涵蓋帶與不帶 usage 兩種回應。
- [x] 3.5 依設計決策「串流路徑要求供應商回報用量」在三個供應商的串流分支加入要求最終片段附上用量的請求選項，鎖定「Streaming calls request usage reporting」。觀察行為：供應商不支援或拒絕該選項時，串流照常完成且用量為空，不失敗也不降級。驗證：新增測試模擬最終片段不含用量的串流，斷言串流成功結束且用量為空。
- [x] 3.6 讓回合結果彙總該回合所有模型呼叫的用量，鎖定「A turn aggregates the usage of its model calls」，含 compaction 自行發出的摘要呼叫；部分呼叫未回報時只加總有回報者，全部未回報時回合用量為空。用量不寫入持久化事件流。驗證：新增測試，腳本化三次模型呼叫其中一次不回報用量，斷言回合用量等於另外兩次的加總。

## 4. 回歸與收尾

- [x] 4.1 重跑 crates/fleety-eval 的離線黃金對話回歸，確認十五筆案例全數通過。觀察行為：黃金案例皆為腳本化短流程且斷言針對呼叫了哪些工具與最終輸出，預期不受本次三項變更影響。驗證：實際執行 fleety-eval 的測試並貼出結果；若有案例漂移，判斷是規格變更的合理後果才更新黃金檔，否則視為迴歸並修正實作。
- [x] 4.2 確認全 workspace 建置與測試通過且未新增 lint 警告。觀察行為：三項變更不引入 unwrap 或 expect，符合 workspace 既有的 clippy 規則。驗證：執行 cargo build --workspace、cargo test --workspace 與 cargo clippy --workspace --all-targets，三者皆無錯誤且無新增警告。
