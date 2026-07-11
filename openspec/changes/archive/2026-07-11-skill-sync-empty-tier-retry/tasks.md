## 1. 空 tier 守門（TDD）

- [x] 1.1 先紅：在 crates/fleety-server/src/skill_sync.rs 測試模組依規格「Syncing is conditional on the repo's latest commit」修訂版的 Example 表格新增測試：同步決策四列（無 SHA→抓、SHA 同且 tier 有 skills→跳過、SHA 異→抓、SHA 同但 tier 空→抓），以及「tier 是否為空」純檢查（目錄不存在→空、只含 SHA 記錄檔→空、含任一子目錄→非空）。驗證：cargo test -p fleety-server skill_sync 出現預期失敗（紅）。
- [x] 1.2 轉綠：實作純函式的空 tier 檢查，並把同步決策改為「SHA 短路僅在 tier 非空時生效」；寫入端不動（重建後照記 SHA）。行為契約：被清空但 SHA 仍最新的 tier 在下一次同步（含開機）自動重抓重建；真正空的來源 repo 每週期重抓、同步不失敗。驗證：1.1 全綠且既有 skill_sync 測試不回歸，cargo test -p fleety-server 綠。

## 2. 文件與真機自癒驗證

- [x] 2.1 文件同步：skill_sync.rs module doc 與 docs/env.md 的 Synced skills 段各補一句「空 tier 即使 SHA 未變也會重新同步（自癒）」。驗證：內容審閱與 delta spec 一致，cargo test -p fleety-server 綠。
- [x] 2.2 真機自癒實測：以真實 TimLai666/skills 的目前 commit SHA 預埋一個「空 tier + 最新 SHA」的故障殘留狀態（synced 目錄只含 SHA 記錄檔），啟動 fleety-server，確認開機同步無視 SHA 相同仍下載重建、synced tier 恢復全部 skills（數量與抽樣核對），log 出現 synced skills updated。驗證：列目錄人工核對。
