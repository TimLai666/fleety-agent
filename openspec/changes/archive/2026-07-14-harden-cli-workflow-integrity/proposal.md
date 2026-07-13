## Why

CLI 稽核確認多條流程會吞掉輸入、印錯 server 錯誤卻 exit 0、把憑證寫進錯誤 profile、在長 OAuth 流程中漂移 target、或在失敗前先留下持久化副作用。這些不是個別文案瑕疵，而是參數、target、持久化與 exit status 沒有共同的交易邊界。

## What Changes

- 所有命令採嚴格 arity 與 flag value 驗證；未知命令、缺值、多餘參數與無效數字回 usage failure，真正 help 維持成功。
- ask 保留所有 positional 文字且附件 flag 缺路徑時失敗；resume/audit 等數字參數不再默默改成預設。
- 所有 `ServerMsg::Error`、業務 `ok=false`、連線提前關閉與 malformed JSON 回非零，不再用空列表或成功狀態掩蓋。
- pair 把 token/fingerprint 寫回實際解析出的 named profile；URL override 不可污染 current profile。
- init 只有 Hello/Welcome 成功後才提交 profile/current；互動 init 找不到 server 或輸入無效時給明確失敗與正確 `pair-code` 指引。
- OAuth 登入全程鎖定 preflight target 與 server fingerprint，callback 有 deadline，server 在等待期間改變時拒絕交付 credential。
- provider TUI 追蹤 dirty state，離開需 Save/Discard/Cancel；狀態文字區分 staged 與 saved；儲存失敗或 conflict 時不得啟動 OAuth。
- ACP 使用完整 resolved URL + token，未知 ACP verb 失敗，resolver 錯誤不 fallback localhost，refresh 保留既有 server 綁定，Zed settings 原子更新。
- `fleetyd` / `fleety-server` 只有無參數或明確 service entry 才啟動 runtime；help 不啟動、unknown 失敗，`daemon up/down` 正規化。
- `fleety update` 聚合 sibling 結果，fleetyd update 失敗使整體失敗；OAuth token 改為原子 owner-only 寫入。
- `fleety server` 各 verb 嚴格拒絕多餘參數與 typo flag；Providers/Models 重複入口合併成符合實際行為的名稱。
- help/version 在任何 migration 或設定 seed 前處理，純查詢不修改使用者檔案；migration 錯誤不再被吞掉。

## Capabilities

### New Capabilities

- `cli-workflow-integrity`: 定義 CLI 參數、target transaction、持久化副作用、互動 dirty state、錯誤 exit status、ACP/OAuth 與 service delegate 的完整性契約。

### Modified Capabilities

(none)

## Impact

- Affected specs: cli-workflow-integrity
- Affected code:
  - Modified:
    - `crates/fleety-cli/src/main.rs`
    - `crates/fleety-cli/src/auth.rs`
    - `crates/fleety-cli/src/acp.rs`
    - `crates/fleety-cli/src/server.rs`
    - `crates/fleety-cli/src/config_panel.rs`
    - `crates/fleety-cli/src/provider_tui.rs`
    - `crates/fleety-cli/tests/cli_smoke.rs`
    - `crates/fleety-daemon/src/main.rs`
    - `crates/fleety-server/src/main.rs`
    - `crates/fleety-tools/src/connection.rs`
    - `crates/fleety-tools/src/config.rs`
    - `crates/fleety-tools/src/oauth.rs`
    - `README.md`
    - `docs/env.md`
  - New: none
  - Removed: none
