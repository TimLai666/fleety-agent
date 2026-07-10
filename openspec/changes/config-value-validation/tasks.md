## 1. Registry validator 機制（fleety-tools）

- [x] 1.1 在 `crates/fleety-tools/src/config.rs` 的 `Setting` struct 加 `validator: Option<fn(&str) -> std::result::Result<(), String>>` 欄位（`fn` 指標保持 `Setting: Copy`），並在 `registry()` 每個既有條目補上此欄位（多數為 `None`）；驗證方式：`cargo build -p fleety-tools` 通過、既有測試 `codex_oauth_settings_registered_with_defaults` 等不回歸。
- [x] 1.2 實作可重用的 validator helper 函式（列舉白名單、布林 `0|1`、非負整數、`http`/`https` URL scheme），錯誤字串內含合法值；為 `Config write value validation` 掛上對應 key：列舉（`FLEETY_POLICY`、`FLEETY_FS_SCOPE`、`FLEETY_VOICE_AUDIO`、`FLEETY_PRESENCE`、`FLEETY_MODEL_EFFORT`/`FLEETY_CHEAP_MODEL_EFFORT`）、布林（`FLEETY_REQUIRE_AUTH`、`FLEETY_AUTO_INSTALL_DEPS`、`FLEETY_FORCE_SSE`、`FLEETY_DISABLE_SSE`）、非負整數（`FLEETY_MODEL_RETRIES`、`FLEETY_MODEL_RETRY_BASE_MS`、`FLEETY_MODEL_RETRY_CAP_MS`、`FLEETY_CMD_TIMEOUT_SECS`、`FLEETY_SSE_TIMEOUT_SECS`、`FLEETY_BACKUP_INTERVAL_SECS`、`FLEETY_PRESENCE_INTERVAL_SECS`、`FLEETY_VOICE_AUDIO_MAX_KB`）、URL（`FLEETY_MODEL_BASE_URL`、`FLEETY_CODEX_AUTHORIZE_URL`、`FLEETY_CODEX_TOKEN_URL`、`FLEETY_CODEX_BACKEND_URL`）；驗證方式：新增單元測試斷言每個掛載 key 的 `validator` 為 `Some` 且合法/非法值分別通過與拒絕。
- [x] 1.3 新增純函式 `pub fn validate(setting: &Setting, value: &str) -> Result<()>`：無 validator 或空字串直接 `Ok(())`（`Config write value validation` 的 pass-through 與 unset 語意），否則呼叫 validator，失敗時包成 `CoreError::Message`，訊息滿足 `Validation error names accepted values`（含 key 名與合法值/scheme）；驗證方式：單元測試 `validate` 對 `FLEETY_VOICE_AUDIO=loud` 的錯誤訊息包含 `auto`/`on`/`off`，對 `FLEETY_MODEL_BASE_URL=notaurl` 的錯誤訊息包含 `http`。

## 2. 寫入路徑接上驗證（fleety-tools）

- [x] 2.1 在 `run_rendered` 的 `Command::Set` 分支，於 `map.insert`/`save` 之前呼叫 `validate(setting, &value)?`，失敗即回錯不寫檔（涵蓋 server/daemon 及 CLI `--target` 遠端共用路徑）；驗證方式：新增測試以臨時 `FLEETY_CONFIG` 對 `run_rendered(["set","FLEETY_REQUIRE_AUTH","abc"])` 斷言回 `Err` 且檔案未被建立/未含該值，對合法值 `require_approval` 斷言成功寫入（對應 `Config write value validation` 的 reject 與 persist 情境）。
- [x] 2.2 在 `edit_line_based` 的 commit 段，`map.insert`/`save` 前呼叫 `validate`，失敗時印出錯誤並 `continue`（不寫檔、保留迴圈），對應 `Config write value validation` 的互動編輯情境；驗證方式：程式碼審閱確認非法值不會呼叫 `save`，並手動以 `config edit`（非 TTY）輸入壞值確認被拒。

## 3. CLI ratatui 編輯路徑（fleety-cli）

- [x] 3.1 在 `crates/fleety-cli/src/config.rs` 的 `on_key` Enter commit（非空值分支）呼叫 `config::validate(setting, &buf)`，失敗時把錯誤訊息寫入 `app.status`、不 `insert`、令 `on_key` 回傳 `false`（不觸發 `save`），保持 `app.edit` 或重置以利重試，滿足 `Config write value validation` 的「interactive edit rejects invalid value without saving」；驗證方式：新增測試建構 `ConfigApp`、對某驗證 key 輸入非法值後按 Enter，斷言 `on_key` 回 `false`、`app.map` 不含該 key、`app.status` 含合法值提示（`Validation error names accepted values`）。
- [x] 3.2 [P] 更新 `crates/fleety-cli/src/config.rs` 既有測試 `config_tui_key_handling` 使用一個無 validator 的 key（如 `FLEETY_TZ` 或 `FLEETY_MODEL`）做 `z` 寫入斷言，避免既有測試因新驗證而失敗；驗證方式：`cargo test -p fleety-cli config` 全綠。

## 4. 迴歸與文件

- [x] 4.1 [P] 執行 `cargo test -p fleety-tools -p fleety-cli` 確認新舊測試全綠、`cargo clippy` 無 `unwrap_used`/`expect_used` 違規；驗證方式：兩指令輸出無錯。
- [x] 4.2 [P] 在 `crates/fleety-tools/src/config.rs` 模組層 doc comment 補一句說明「registry 條目可帶 validator，`config set`/edit 寫入前驗證，未掛 validator 的 key 放行」；驗證方式：內容審閱。