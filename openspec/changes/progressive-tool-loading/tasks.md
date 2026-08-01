## 1. registry 的啟用集合

- [x] 1.1 依設計決策「啟用集合放在 registry，預設為「全部啟用」」在 ToolRegistry 加入可選啟用集合，鎖定「An unset activation state preserves existing behavior」與「The model is shown a resident tool set, not the whole surface」的篩選面：未設定時 specs() 回傳全部工具，設定後只回傳集合內工具，集合含未註冊名稱時忽略之。觀察行為：subagent、workflow、eval、daemon 等未設定的呼叫端行為完全不變。驗證：新增 registry 單元測試三例（未設定等同全部、設定後只回傳集合內、未知名稱被忽略），並確認 crates/agent-core 既有測試全數通過。
- [x] 1.2 依設計決策「工具呼叫仍以「已註冊」為準，不以「已啟用」為準」確認並以測試釘住 ToolRegistry::call 不查詢啟用集合，鎖定「Activation is a context budget, never an authorization boundary」。觀察行為：未啟用但已註冊的工具仍可被呼叫，拒絕與否只由風險分級與核可閘門決定。驗證：新增測試呼叫一個不在啟用集合內的已註冊工具，斷言呼叫成功而非回報找不到工具。

## 2. 啟用在同一回合生效

- [x] 2.1 依設計決策「迴圈每一步重新讀取工具清單」把 agent 迴圈取得工具 spec 的位置從迴圈外移入迴圈內，鎖定「Activation takes effect within the same turn」。觀察行為：模型於某一步啟用群組後，同一回合的下一次模型呼叫即可看到並呼叫該群組工具；風險查詢使用的清單同步更新。驗證：新增迴圈測試，以一個會在被呼叫時擴充啟用集合的假工具，斷言下一步的模型請求已含新工具；並確認 agent-core 既有迴圈與 compaction 測試全數通過。

## 3. 群組定義與工具搜尋

- [x] 3.1 依設計決策「啟用以群組為粒度」建立群組定義表，涵蓋 registry 全部 80 個工具且無遺漏，並標出常駐集合（檔案與命令核心十個、skills 兩個入口、核心記憶讀寫兩個、跨裝置兩個入口）。觀察行為：每個註冊工具恰屬於一個群組。驗證：新增測試，取完整 registry 的工具名稱集合與群組定義表比對，斷言雙向無差集（沒有工具未分組、沒有群組列出不存在的工具）。
- [x] 3.2 實作工具搜尋工具，鎖定「The model can discover and activate further tools by capability」：以能力描述查詢，回傳命中群組名稱與其中工具名稱及一行摘要，並把該群組加入當前對話啟用集合；未命中時明確回報且不改變集合；重複啟用不重複也不報錯。觀察行為：模型能自行從常駐集合抵達任何延後群組。驗證：新增測試三例（命中並啟用、未命中且集合不變、重複查詢無副作用），使用規格範例中的瀏覽器情境作為命中案例。
- [x] 3.3 讓工具搜尋入口恆為常駐且不可停用，鎖定「The model is shown a resident tool set, not the whole surface」的第二個情境。觀察行為：任何啟用狀態下（含自儲存載入的）模型都看得到搜尋入口。驗證：新增測試，載入一個不含搜尋入口的啟用集合，斷言 specs() 仍包含它。

## 4. 對話持久化

- [x] 4.1 依設計決策「啟用狀態隨對話持久化」讓啟用集合隨對話儲存與載入，鎖定「Activation persists for the conversation」。觀察行為：啟用過的群組在後續回合與重啟後仍可用，模型不需重複搜尋。驗證：新增測試寫入啟用集合後重新載入該對話，斷言集合內容一致。
- [x] 4.2 讓損壞或過期的啟用狀態安全降級，鎖定「Activation persists for the conversation」的降級情境。觀察行為：無法解析的狀態視為未設定（退回全部啟用），含未註冊名稱時忽略該名稱，兩者都不中斷對話。驗證：新增測試兩例，分別以無法解析的內容與含未知工具名的集合，斷言對話仍能正常取得工具清單。

## 5. 提示與收尾

- [x] 5.1 在 prompts/protocol.md 加入極短說明，告知模型目前看得到的工具不是全部、需要其他能力時應先搜尋。觀察行為：新增文字長度應遠小於本變更省下的 schema 位元組，否則收益被抵銷。驗證：內容審閱確認段落不超過數行，並記錄新增字元數與省下位元組數的對比。
- [x] 5.2 重跑 measure_harness_footprint 與 measure_tool_schema_wire_size，記錄開場工具 schema 位元組數，鎖定本變更的量化目標。觀察行為：開場工具 schema 顯著低於變更前的 36,927 bytes。驗證：以 --nocapture 執行兩個量測測試並在本任務下記錄變更前後數字。
- [ ] 5.3 以真實模型手動驗證至少一個需要延後工具的情境（例如要求代理操作瀏覽器），確認它會先搜尋而非回報做不到，鎖定設計風險「模型不知道要搜尋」。驗證：記錄該次對話中模型是否呼叫了搜尋工具、是否成功完成任務；若模型未搜尋，視為 protocol.md 說明不足並修正後重測。
- [x] 5.4 確認全 workspace 建置、測試與 clippy 無新增錯誤或警告。觀察行為：本變更不引入 unwrap 或 expect。驗證：執行 cargo build --workspace、cargo test --workspace、cargo clippy --workspace --all-targets，並與變更前的既有失敗清單比對（fleety-cli 那組漫遊測試屬既有問題，不計入）。
