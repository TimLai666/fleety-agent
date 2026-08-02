## 1. Server 雙棧伴聽器

- [x] 1.1 先寫失敗測試，鎖定「Server bootstrap configuration」的伴聽器情境：抽出一個可單測的綁定函式（輸入設定位址字串，回傳一至兩個 listener），斷言 `0.0.0.0:<臨時埠>` 與 `127.0.0.1:<臨時埠>` 在 v6 可用時各回傳兩個 listener（v4 + 同埠 v6 對應位址）、明確位址（例如 `192.0.2.1:9` 形式不實際綁，改用可綁的具體位址如 `127.0.0.1` 以外的迴環別名或以字串判斷分支）只回傳一個、v6 埠被占時仍成功回傳單一 v4 listener。驗證：測試在實作前失敗、1.2 完成後通過。
- [x] 1.2 在 crates/fleety-server/src/main.rs 實作該綁定函式並讓 serve 迴圈同時服務全部 listener（伴聽器接受的連線走完全相同的 router），v6 綁定失敗記一行日誌不失敗，鎖定「a failed companion bind degrades to IPv4-only」與「an explicit address is bound exactly」。觀察行為：預設啟動後 `127.0.0.1` 與 `::1` 同埠皆可連。驗證：1.1 全綠，並新增一個整合測試以預設形式位址啟動、實際從 `[::1]` 建立 TCP 連線成功。

## 2. 客戶端 localhost 撥號偏好 v4

- [x] 2.1 先寫失敗測試，鎖定「Dialing a localhost endpoint prefers IPv4」：對撥號正規化函式斷言 `ws://localhost:8787` 改撥 `127.0.0.1:8787`、`wss://localhost:9/path` 保留 scheme 埠與路徑、`ws://[::1]:8787` 與 `ws://myhost:8787` 與 `ws://127.0.0.1:8787` 逐字不變。驗證：測試在實作前失敗、2.2 完成後通過。
- [x] 2.2 在 crates/fleety-tools/src/transport.rs 的撥號單一路口（connect / connect_secure 共同下游）套用該正規化，僅影響 socket 連往哪裡，錯誤訊息與回報中的 URL 維持使用者拼法。觀察行為：CLI 一次性連線與 Daemon 重連迴圈同時受惠，無須各自改動。驗證：2.1 全綠；新增一個測試只綁 `127.0.0.1` 的 listener，以 `ws://localhost:<port>` 完成 WebSocket 開啟，證明不再依賴解析器的落回。

## 3. 文件與登錄表同步

- [x] 3.1 依變更完整性規則同步 `FLEETY_*` 的四個表面：docs/env.md 的 FLEETY_ADDR 條目補伴聽器語意與 v6-only 需拼 `[::1]` 的說明；crates/fleety-tools/src/config.rs registry 中 FLEETY_ADDR 的說明文字一致更新；README 若有提及監聽位址則核對（無則記錄免改）；AGENTS.md 把 2026-08-02 的 localhost/雙棧 follow-up 標記為 resolved 並指向本 change。驗證：內容審閱四處說法一致，grep 確認無其他 FLEETY_ADDR 文件點遺漏。

## 4. 收尾驗證

- [x] 4.1 確認全 workspace 建置、測試與 clippy 無新增錯誤或警告，且不引入 unwrap/expect。驗證：執行 cargo build --workspace、cargo test --workspace、cargo clippy --workspace --all-targets 並與變更前基線比對（目前基線為全綠）。
