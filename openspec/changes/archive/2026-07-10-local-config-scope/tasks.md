## 1. fleety-tools scope 過濾(fleety-tools 加 scope 過濾的 run 變體與 same-scope 守衛)

- [x] 1.1 在 crates/fleety-tools/src/config.rs 新增 `LOCAL_SCOPES`(Cli+Shared)、`rows_in_scopes(map, &[Scope])`、`ensure_scope(key, &[Scope]) -> Result<()>`(非該 scope 的已知 key 回錯並指向正確編輯處),交付「The local CLI config surface is scoped to this device's settings」的 fleety-tools 面。單元:`rows_in_scopes` 只回指定 scope、`ensure_scope` 對 Server key(FLEETY_ADDR)回錯指向遠端、對 Cli/Shared key 放行。
- [x] 1.2 把 `run_rendered` 重構委派 `run_rendered_scoped(args, Option<&[Scope]>)`、`run` 委派 `run_scoped(args, Option<&[Scope]>)`:`None` 不過濾(server/daemon 現況)、`Some(sc)` 時 List 用 `rows_in_scopes`、Get/Set/Unset 先 `ensure_scope`;既有 `run`/`run_rendered` 簽章不變。單元:`run_rendered_scoped(list, Some(LOCAL_SCOPES))` 不含 Server key、`set FLEETY_ADDR`(Some LOCAL_SCOPES)被拒不落檔、`set FLEETY_TZ` 放行;`run_rendered(None)` 仍列全部(server 不受影響)。

## 2. CLI local 路徑套用(CLI local 路徑改用 scoped run + 互動 rows 過濾)

- [x] 2.1 在 crates/fleety-cli/src/config.rs 把 fallthrough 由 `fleety_tools::config::run(args)` 改為 `run_scoped(args, Some(config::LOCAL_SCOPES))`、`build_rows` 只收 Cli/Shared scope,交付「The local CLI config surface is scoped to this device's settings」的 CLI 面(local list/edit 只本機設定)。單元:`build_rows` 只含 Cli/Shared;以既有 CLI config 測試確認互動 rows 不含 Server key。

## 3. 驗證

- [x] 3.1 全 workspace 回歸:`cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-cli` 無新違規;手動 `fleety config --target local list` 只見 Cli/Shared、`--target local set FLEETY_ADDR …` 被拒、`fleety-server config list` 仍列全部。驗證:命令輸出 + 手動檢查。
