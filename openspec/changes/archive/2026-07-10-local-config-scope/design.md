## Context

現況(讀程式碼驗證):`fleety-tools` 的 config.rs 有 `Scope { Server, Daemon, Cli, Shared }`、`rows(map)`(列 registry 全部)、`run_rendered(args)`(List 用 rows、Get/Set/Unset 不分 scope)、`run(args)`(印 rendered,或 `edit` 走 line-based)。`config_apply`(server 遠端)用 `run_rendered`。CLI 的 config.rs:`run(args)` 對 `provider edit`/`edit` 開 ratatui、否則 fallthrough `fleety_tools::config::run(args)`;`build_rows(map)` 也列 registry 全部;CLI 的 local 路徑(main.rs `--target local` 或互動 edit)一律走 CLI `config::run`。故 CLI local 目前顯示所有 scope、可寫任何 key。

## Goals / Non-Goals

**Goals:** CLI 的 local config(`--target local` 與互動 edit)只顯示 Cli/Shared;local get/set/unset 一個 Server/Daemon key 被拒並指向遠端路徑。

**Non-Goals:**（見 proposal）server/daemon 自機 config 顯示、遠端 server config 面、Phase 2 面板。

## Decisions

### fleety-tools 加 scope 過濾的 run 變體與 same-scope 守衛

新增 `pub const LOCAL_SCOPES: &[Scope] = &[Scope::Cli, Scope::Shared];`、`rows_in_scopes(map, scopes)`(rows 的 scope 過濾版)、`ensure_scope(key, scopes) -> Result<()>`(key 不在 scopes 內回明確錯誤,指向該 scope 的正確編輯路徑)。`run_rendered` 重構為委派 `run_rendered_scoped(args, scopes: Option<&[Scope]>)`:`None`=不過濾(server/daemon 現況),`Some(sc)`=List 用 `rows_in_scopes`、Get/Set/Unset 先 `ensure_scope`。同理 `run` 委派 `run_scoped(args, Option<&[Scope]>)`。既有 `run`/`run_rendered` 簽章不變(委派 None),server `config_apply` 不受影響。

### CLI local 路徑改用 scoped run + 互動 rows 過濾

CLI config.rs 的 fallthrough 由 `fleety_tools::config::run(args)` 改為 `run_scoped(args, Some(LOCAL_SCOPES))`。`build_rows` 改為只收 Cli/Shared scope(互動 edit 螢幕只列本機設定)。因 CLI `config::run` 只在 local 路徑被呼叫(main.rs `--target local` 或 interactive edit),此限制只作用於 local。

## Implementation Contract

**行為:**
- `fleety config --target local list` 只列 Cli/Shared(例:FLEETY_VOICE_AUDIO、FLEETY_TZ),不列 Server key(FLEETY_ADDR/FLEETY_POLICY/FLEETY_MODEL_KEY)。
- `fleety config --target local set FLEETY_ADDR 0.0.0.0:8787` → 被拒,訊息指「這是 server 設定,用 `fleety config set FLEETY_ADDR …`(預設連到 server)」。
- `fleety config --target local set FLEETY_TZ Asia/Taipei` → 照舊寫入本機 config.toml。
- 互動 `fleety config`(TTY)edit 螢幕只列 Cli/Shared 列。
- `fleety-server config list` / `fleetyd config list`(各自機)照舊列自機所有(不過濾)。

**介面 / 資料形狀:**
- `fleety_tools::config`:`LOCAL_SCOPES`、`rows_in_scopes(&ConfigMap, &[Scope])`、`ensure_scope(&str, &[Scope]) -> Result<()>`、`run_scoped(&[String], Option<&[Scope]>)`、`run_rendered_scoped(&[String], Option<&[Scope]>)`;`run`/`run_rendered` 委派 `None`。
- CLI config.rs:fallthrough 用 `run_scoped(args, Some(LOCAL_SCOPES))`;`build_rows` 過濾 Cli/Shared。

**失敗模式:**
- local set/get/unset 一個非 Cli/Shared 的已知 key → 明確錯誤指向遠端路徑;未知 key → 既有 unknown setting 錯誤。

**驗收條件:**
- fleety-tools 單元:`rows_in_scopes` 只回指定 scope、`ensure_scope` 對 Server key 回錯指向遠端 / 對 Cli 放行、`run_rendered_scoped(Some(LOCAL_SCOPES))` 的 list 不含 Server key、set FLEETY_ADDR 被拒。
- fleety-cli 單元:`build_rows` 只含 Cli/Shared。
- `cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15、`cargo clippy -p fleety-tools -p fleety-cli` 無新違規。

**範圍邊界:**
- In scope:CLI local config 的 scope 過濾 + same-scope 守衛 + 互動 rows 過濾。
- Out of scope:server/daemon 自機 config、遠端 server config 面、Phase 2 面板。

## Risks / Trade-offs

- [server/daemon 誤被過濾] `run`/`run_rendered` 委派 `None`,只有 CLI local 傳 `Some(LOCAL_SCOPES)`,server `config_apply`/daemon 不變。
- [使用者本想本機設某 Server key] 那本來就是無效編輯(server 讀自機 config);守衛把它導向正確遠端路徑,是修正非退步。
- [Shared key 兩處可改] Shared 同時在 local 與 server 面出現是刻意(colocation 時單一權威來源另議,見既有 M7 討論);此 change 不改 Shared 語意。
