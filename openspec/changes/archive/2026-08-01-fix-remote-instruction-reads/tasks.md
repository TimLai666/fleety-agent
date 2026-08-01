## 1. 反向行號函式

- [x] 1.1 在 crates/fleety-tools/src/lib.rs 新增公開的反向行號函式，鎖定「A line-numbered view can be reversed to the original text」：移除每行行號前綴至第一個定位字元為止，無定位字元的行原樣保留。驗證：新增測試涵蓋規格的 round-trip 範例（含內容本身帶定位字元的那一行）與無定位字元的行，斷言還原結果與原文逐字相同。

## 2. 修正兩條遠端讀取路徑

- [x] 2.1 修正 Claude hooks 探索路徑：傳給 read_file 的參數鍵由 file 改為 path，取回結果後以 1.1 的函式還原再解析 JSON。觀察行為：`.claude/settings.json` 能被實際讀到並解析出 hooks，而非靜默取得空結果。驗證：新增測試以一份含 hooks 的 settings.json 走該路徑，斷言解析出的 hooks 非空；並斷言若鍵名寫錯則測試失敗（不得靜默通過）。
- [x] 2.2 修正跨裝置指令檔注入路徑：device_exec 內層傳給 read_file 的參數鍵由 file 改為 path，取回結果後以 1.1 的函式還原再送進既有長度上限處理。觀察行為：跨裝置來源對話的 AGENTS.md / CLAUDE.md 內容確實進入 context 且不含行號前綴，符合 instruction-file-injection 規格的跨裝置情境。驗證：新增測試以假的 device_exec 回應（加行號視圖）走該路徑，斷言注入內容等於原始檔案文字。

## 3. 收尾驗證

- [x] 3.1 確認全 workspace 建置、測試與 clippy 無新增錯誤或警告。觀察行為：本次不引入 unwrap 或 expect，符合 workspace 既有規則。驗證：執行 cargo build --workspace、cargo test --workspace、cargo clippy --workspace --all-targets，並與修改前的既有失敗清單比對，確認沒有新增失敗（fleety-cli 那組時序不穩的握手測試屬既有問題，不計入）。
