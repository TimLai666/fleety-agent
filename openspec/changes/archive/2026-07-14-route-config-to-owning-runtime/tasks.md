<!-- 每項任務都包含可觀察行為與驗證目標。 -->

## 1. 設定擁有權與持久化基礎

- [x] 1.1 以 TDD 新增 Scope determines the owning runtime：在 `fleety-tools` 定義 `CLI_SCOPES`、`DAEMON_SCOPES`、`SERVER_SCOPES`、owner lookup 與 owner-scoped strict dispatch，使 The local CLI config surface is scoped to this device's settings，並以 `server_scopes_exclude_foreign_keys`、`daemon_scopes_exclude_foreign_keys`、`cli_scopes_exclude_foreign_keys` 證明 foreign set/unset 不改檔。
- [x] 1.2 以 TDD 實作 Mutations use strict reads and cross-process locks 並滿足 Config mutation rejects corrupt input and no-op identity keys：損壞 TOML 必須拒絕 mutation、並行更新不得 lost update、`FLEETY_DEVICE_ID` 不再是假設定鍵；以 `mutating_corrupt_config_never_overwrites`、`concurrent_config_mutations_preserve_both_values`、`device_id_is_not_a_registry_setting` 驗證。
- [x] 1.3 以 TDD 修正 Config changes apply atomically under optimistic locking：同一個 server `ConfigApply` 的 flat changes 與 `providers_json` 必須先全部驗證再落地，provider snapshot 壞檔不得變 `{}`；以 `structured_apply_is_all_or_nothing_across_config_and_providers`、`legacy_config_exec_rejects_corrupt_config_without_write`、`provider_snapshot_rejects_corrupt_file` 驗證兩份檔案 bytes 不變。

## 2. Daemon owner 控制路徑

- [x] 2.1 實作 Device configuration reuses the daemon tool bridge 與 Device targets are executed by fleetyd：fleetyd 處理未公開的 config exec/snapshot/apply 保留操作，只允許 Daemon/Shared scope、revision conflict 不落地；以 `daemon_config_exec_is_scoped`、`daemon_config_snapshot_excludes_foreign_scopes`、`daemon_config_apply_rejects_stale_revision` 驗證。
- [x] 2.2 實作 Only daemon-capable sessions occupy the routing hub 與 Daemon routing is not displaced by interactive sessions：只有成功廣告 local tools 的 session 可註冊 Hub，cleanup 只移除自己的 sender；以 `cli_session_does_not_replace_daemon_route`、`cli_disconnect_keeps_daemon_route`、`stale_disconnect_keeps_replacement` 驗證。
- [x] 2.3 在 server 端完成 `ConfigTarget::Device` 的 `ConfigExec`/`ConfigSnapshot`/`ConfigApply` 路由與錯誤轉換，使 disconnected daemon 明確失敗且 Routing failures never fall back to config files；以 `device_config_routes_to_daemon`、`device_config_offline_returns_error`、`device_config_bad_reply_returns_error` 驗證。

## 3. CLI owner 路由與命令 UX

- [x] 3.1 以 TDD 實作 Config command routing is automatic and explicit targets are owner selectors，使 CLI configuration routes by owning runtime 與 Explicit targets enforce ownership：Auto 依 key scope 路由、daemon 解析 current device id、local 僅為 cli alias、provider/model 固定 server；以 `owner_route_matrix`、`parse_config_targets`、`target_owner_mismatch_fails_before_io` 驗證。
- [x] 3.2 實作 Failures and usage are process failures 與 Usage and command failures are machine-detectable：`ConfigResult ok=false`、`ServerMsg::Error`、malformed target、未知/缺參數回非零，help 回零，daemon up/down 正規化且未知 fleetyd 命令不得啟動前景服務；以 CLI smoke 的 `config_rejection_is_nonzero`、`unknown_commands_are_nonzero`、`help_is_zero`、`daemon_aliases_are_bounded` 驗證。
- [x] 3.3 移除 CLI 本機 provider 寫檔路徑，使 An interactive screen manages providers on a TTY 永遠由 server snapshot/apply 持久化，cli/local target 在 editor 前拒絕；以 `provider_edit_local_target_is_rejected_without_file_change` 與既有 remote editor tests 驗證。

## 4. 四區互動面板

- [x] 4.1 實作 The interactive panel has four owner regions，並更新既有 Bare fleety config opens a three-region interactive panel 契約為四區行為：Connection / CLI / Daemon / Server 導覽、資料與提示分離；以 `panel_cycles_four_regions` 與 headless render assertions 驗證標題與鍵盤流程。
- [x] 4.2 實作 Daemon and server regions persist only through their owners：兩區各自保存 availability、revision、entries、staged changes 與 apply target，任一 owner 不可用時只標示 unavailable、不寫檔；以 `daemon_and_server_staging_are_isolated`、`daemon_unavailable_keeps_other_regions`、`server_unavailable_never_local_applies` 驗證。

## 5. 端到端與文件

- [x] 5.1 擴充 CLI、server、fleetyd smoke tests，證明 server key 只到 server、daemon/shared key 只到 fleetyd、Cli key 只到 CLI、RPC failure 不改 `config.toml`/`providers.toml`，並以 `cargo test -p fleety --test cli_smoke`、`cargo test -p fleetyd --test fleetyd_smoke`、`cargo test -p fleety-server --test server_smoke` 驗證。
- [x] 5.2 更新 `README.md`、`docs/env.md`、`docs/design-cli-config.md` 與 command help，使 owner matrix、`--target server|daemon|cli|device`、四區面板、無 fallback、daemon 離線限制與升級指引一致；以 `rg` 比對舊的三區、local Cli/Shared、local provider fallback 說法並執行 `spectra analyze route-config-to-owning-runtime --json` 驗證。
- [x] 5.3 執行完整品質閘：`spectra validate route-config-to-owning-runtime`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace -- --test-threads=1`，並手動啟動 fleety-server + fleetyd 驗證 server/daemon 各自收到設定且停止其中一方時沒有檔案 fallback。
