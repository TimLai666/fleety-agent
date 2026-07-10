## 1. Extend ProviderEditor model methods

- [x] 1.1 在 `crates/fleety-cli/src/provider_tui.rs` 的 `ProviderEditor` 新增 `set_provider`，以 `config provider set` 的「只改有給欄位、其餘保留」語意就地更新既有 provider（provider 不存在時回傳指名錯誤），交付「Interactive screen edits an existing provider in place」的模型層；在同檔以單元測試編輯一個被 group 引用的 provider 的 model，斷言未改欄位保留且 group 綁定不變。
- [x] 1.2 在 `ProviderEditor` 新增 `remove_group`（仍被某 role 引用時拒絕並指名引用的 role）與 `unset_role`（未定義的 role 回傳錯誤），交付「Interactive screen removes groups and unsets roles」的模型層；以單元測試覆蓋被引用 group 遭拒與成功 unset role 兩路徑。

## 2. Per-field input and validation

- [x] 2.1 以逐欄提示流程（name → base_url → model → key）取代 `submit` 內 AddProvider 的逗號單行解析，必填欄位（name／base_url／model）留空時以指名該欄位的錯誤擋下，交付「Interactive screen validates provider fields per field」；以 `on_key` 狀態機單元測試斷言空 model 被拒、完整序列成功新增 provider。

## 3. Screen actions: edit and guarded delete

- [x] 3.1 新增 `e` 編輯動作，把選取 provider 的欄位載入逐欄流程並透過 `set_provider` 存檔，使被引用的 provider 免解綁即可改，交付「Interactive screen edits an existing provider in place」的 UI 路徑；單元測試斷言編輯選取 provider 只更動被改欄位。
- [x] 3.2 將 `d` 刪除改為先進入指名該 provider 的確認提示，接受才移除、取消則配置不變，交付「Interactive screen confirms provider deletion」；單元測試斷言單一 `d` 不會移除，且 confirm／cancel 行為正確。
- [x] 3.3 在 Browse 鍵盤映射加入 group remove 與 role unset 動作（接到 1.2 的新方法），並把 guard 錯誤呈現在狀態列，交付「Interactive screen removes groups and unsets roles」的 UI 路徑；以 `on_key` 單元測試覆蓋按鍵處理與被引用 group 遭拒的狀態訊息。

## 4. Help line and verification

- [x] 4.1 更新 Browse 狀態／說明字串，讓 edit、group remove、role unset 新按鍵可見；執行 `cargo test -p fleety-cli provider_tui` 與 `cargo clippy -p fleety-cli` 確認狀態機測試通過且無 clippy 警告。