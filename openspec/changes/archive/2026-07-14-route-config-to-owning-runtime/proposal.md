## Why

目前 CLI 把設定的「所在主機」與「執行元件擁有者」混在一起：本機目標會直接寫共用設定檔，遠端 server handler 又未依 scope 限制，因此 daemon 設定可能被寫到錯誤主機，provider 還保留直接編輯本機 server 檔案的 fallback。部分遠端拒絕與錯誤只印訊息卻回傳成功 exit code，使用者與腳本都可能誤判設定已生效。

完整 CLI 稽核另確認 structured apply 可能失敗卻已部分寫入、損壞設定可能被 fail-soft 覆蓋，以及 `FLEETY_DEVICE_ID` 看似可設定但不會成為實際身分來源。其餘非設定路由的 CLI 問題另立 change，避免把無關子系統混成一包。

## What Changes

- `fleety config get/set/unset` 依 registry scope 自動路由：Server 與 provider/model 送到 server，Daemon 與 Shared 送到目標裝置上正在執行的 fleetyd，Cli 僅由 CLI 自己處理。
- 新增明確的 `--target server|daemon|cli|<device-id>` 語意；保留 `local` 相容別名但只代表 CLI，且錯誤目標會被拒絕，不再偷偷改另一個元件的檔案。
- 裸 `fleety config` 的設定面板改成 Connection / CLI / Daemon / Server 四區；daemon 與 server 區都從實際 owner 取得 snapshot 並送回 apply，連線失敗時維持唯讀或明確失敗，絕不 fallback 到檔案。
- `config provider edit` 永遠作用於連線中的 server；移除 `--target local` 直接寫 `providers.toml` 的 CLI 路徑。
- server 只接受 Server scope 的設定命令；Device target 經既有 daemon 長連線轉交 fleetyd 執行，且一般 CLI session 不再覆蓋同 device id 的 daemon 路由。
- 設定 mutation 改用 strict load，server 的 flat changes 與 providers write-back 先全數驗證再寫入；損壞檔案與 provider snapshot 失敗不再降級成空設定。
- 遠端 `ConfigResult` / `Error`、usage 與未知命令回傳正確非零 exit code；修正 `daemon up/down` 與未知 fleetyd 命令會意外進入前景 daemon 的流程。
- `FLEETY_DEVICE_ID` 從 config registry 移除，裝置身分只由 `connections.toml` 管理；config read-modify-write 增加跨程序鎖，避免 CLI 與 daemon 更新不同 scope 時 lost update。

## Capabilities

### New Capabilities

- `owner-routed-configuration`: 定義 CLI 設定命令依元件擁有者路由、失敗不落地、strict mutation 與一致 exit status 的契約。

### Modified Capabilities

- `structured-config-protocol`: Device target 從未支援改為透過連線中的 fleetyd 執行 snapshot、apply 與文字命令，並補齊跨設定檔的 all-or-nothing。
- `interactive-config-panel`: 三區改為明確的 Connection / CLI / Daemon / Server 四區，daemon/server 不可直接寫檔或 fallback。
- `local-config-scope`: `local` 不再包含 Shared 或任何 daemon/server 設定，只保留 CLI owner。
- `provider-config-surface`: 移除 CLI 的本機 `providers.toml` 編輯模式，provider/model 一律由 server 持久化。
- `device-registry-and-routing`: daemon-capable session 必須維持唯一可路由位址，不可被同 device id 的一般 CLI session 覆蓋。

## Impact

- Affected specs: owner-routed-configuration, structured-config-protocol, interactive-config-panel, local-config-scope, provider-config-surface, device-registry-and-routing, acp-adapter, connection-profiles, server-credential-store
- Affected code:
  - Modified:
    - `crates/fleety-tools/src/config.rs`
    - `crates/fleety-protocol/src/lib.rs`
    - `crates/fleety-cli/src/main.rs`
    - `crates/fleety-cli/src/config.rs`
    - `crates/fleety-cli/src/config_panel.rs`
    - `crates/fleety-cli/src/server.rs`
    - `crates/fleety-cli/src/auth.rs`
    - `crates/fleety-cli/tests/cli_smoke.rs`
    - `crates/fleety-daemon/src/main.rs`
    - `crates/fleety-daemon/tests/fleetyd_smoke.rs`
    - `crates/fleety-server/src/main.rs`
    - `crates/fleety-server/src/conn.rs`
    - `crates/fleety-server/src/bridge.rs`
    - `README.md`
    - `docs/env.md`
    - `docs/design-cli-config.md`
  - New: none
  - Removed: none
