## Why

`config set` 與互動編輯只用 `find(&key)` 驗 key，不驗 value。寫壞的值（`FLEETY_REQUIRE_AUTH=abc`、`FLEETY_AUTO_INSTALL_DEPS=false`、壞掉的 base URL）會被寫進 `config.toml`，下次 boot 被消費端的 silent fallback 吃掉（`FLEETY_FS_SCOPE` 非 `workspace` 一律當 full、`FLEETY_AUTO_INSTALL_DEPS` 非 `0` 一律當 on、`FLEETY_CMD_TIMEOUT_SECS` parse 失敗退回預設），使用者完全不知道設定沒生效。

## What Changes

- 在 `fleety_tools::config::Setting` 加一個可選 validator 欄位（`fn` 指標，保持 `Setting: Copy`），並在 `registry()` 為值域明確的 key 掛上 validator：列舉白名單（`FLEETY_POLICY`、`FLEETY_FS_SCOPE`、`FLEETY_VOICE_AUDIO`、`FLEETY_PRESENCE`、`FLEETY_MODEL_EFFORT`/`FLEETY_CHEAP_MODEL_EFFORT`）、布林 `0|1`（`FLEETY_REQUIRE_AUTH`、`FLEETY_AUTO_INSTALL_DEPS`、`FLEETY_FORCE_SSE`、`FLEETY_DISABLE_SSE`）、非負整數（`FLEETY_MODEL_RETRIES`、`FLEETY_MODEL_RETRY_BASE_MS`、`FLEETY_MODEL_RETRY_CAP_MS`、`FLEETY_CMD_TIMEOUT_SECS`、`FLEETY_SSE_TIMEOUT_SECS`、`FLEETY_BACKUP_INTERVAL_SECS`、`FLEETY_PRESENCE_INTERVAL_SECS`、`FLEETY_VOICE_AUDIO_MAX_KB`）、URL scheme（`FLEETY_MODEL_BASE_URL`、`FLEETY_CODEX_AUTHORIZE_URL`/`FLEETY_CODEX_TOKEN_URL`/`FLEETY_CODEX_BACKEND_URL` 需 `http(s)://`）。
- 新增一個純函式 `validate(setting, value) -> Result<()>`，驗證失敗回傳的 `CoreError::Message` 內含合法值說明。
- 三個寫入路徑一律先過同一個 `validate` 再落地：`run_rendered` 的 `Command::Set`（server/daemon/CLI 遠端共用）、`edit_line_based`（非 TTY 行編輯）、`fleety-cli` ratatui `on_key` 的 Enter commit。驗證失敗不寫檔並回報錯誤（TUI 顯示在狀態列並保持編輯狀態）。
- 沒有掛 validator 的 key（如 `FLEETY_TZ`、`FLEETY_ADDR`、`FLEETY_MODEL` 名稱類）維持放行，行為不變。

## Non-Goals

- 不在 boot 載入（`load`/`seed_env_from_config`）時對既有 `config.toml` 做回溯驗證或警告；本次只在寫入時把關（既有壞值仍由消費端 fallback 處理）。
- 不改動任何消費端的 fallback 行為（`fs_confined`、`auto_install_enabled`、`command_timeout` 等）。
- 不驗證 `FLEETY_TZ` 的 IANA 正確性（需時區資料庫，超出本次範圍，維持放行）。
- 不驗證 `providers.toml` 的 provider/group/role 值（本次只涵蓋 flat registry 的 `config set`/`edit`）。
- 不做值的正規化或自動修正（例如把 `TRUE` 轉成 `1`）；只做接受或拒絕。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- runtime-configuration: 新增「config 寫入時對值做 registry validator 驗證、拒絕非法值、錯誤訊息列出合法值、未掛 validator 的 key 放行」的規範。

## Impact

- Affected specs: runtime-configuration
- Affected code:
  - Modified: crates/fleety-tools/src/config.rs
  - Modified: crates/fleety-cli/src/config.rs