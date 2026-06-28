<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。互動式 TUI 的實際鍵盤操作為環境相依，需手動驗證。 -->

## 1. 共用設定核心（fleety-tools）

- [x] 1.1 [P] 在 crates/fleety-tools 新增 config 模組：`Setting { key, scope, default, description, secret }`、`enum Scope { Server, Daemon, Cli, Shared }`、`registry()`、`config_path()`（FLEETY_CONFIG 覆寫，否則 ~/.fleety/config.toml）、`load/save`（TOML，(Scope,key) map）、`resolve(key,&map) -> { value, source: Env|Config|Default }`、`seed_env_from_config`（只設未設定的 env）、以及顯示遮罩 helper——交付 "Read precedence is environment, then config, then default"、"Settings persist to a config file consumed at boot" 的核心（決策「A single typed setting registry shared across binaries」「Persisted to `~/.fleety/config.toml`, sectioned by scope」「Read precedence: env then config then default」「Secrets are stored but masked in display」）。驗證:單元測試 env>config>default 與 source 正確、未知 key 被拒、secret 遮罩、load/save round-trip（temp path）、壞 TOML→空且不 panic;cargo test -p fleety-tools 全綠。

## 2. CLI 命令與互動介面

- [x] 2.1 在 crates/fleety-cli 新增 config 子命令 list/get/set/unset（用 1.1 的 registry/resolver/load/save；list 顯示 scope+值+來源、secret 遮罩；set 驗證 key 後寫入；未知 key 回可行動錯誤）並於 main.rs 接線——交付 "Settings are discoverable and editable from the terminal"（決策「`fleety config` commands + an interactive TUI editor」）。驗證:命令→動作的 dispatch 純邏輯單元測試（不需 TTY）;cargo build -p fleety-cli 綠。
- [x] 2.2 在 crates/fleety-cli 新增互動式設定畫面 `fleety config edit`（沿用既有 ratatui TUI；依 scope 列出設定、選取後於欄位編輯、儲存走與 set 相同的驗證寫入；list 視圖遮罩 secret、僅編輯該欄時顯示）——交付 "Interactive settings editor"（決策「`fleety config` commands + an interactive TUI editor」「Secrets are stored but masked in display」）。驗證:設定列表視圖建構 smoke test（不需互動斷言）;cargo build -p fleety-cli 綠;實際鍵盤操作標為手動驗證。

## 3. server / daemon 啟動消費

- [x] 3.1 crates/fleety-server/src/main.rs 與 crates/fleety-daemon/src/main.rs 啟動早期載入 config 並 `seed_env_from_config`（只填未設定的 env，env 永遠優先），使既有 std::env 讀取點自動套用 config 值；壞/缺 config 只 warn 繼續——交付 "Settings persist to a config file consumed at boot"（決策「Read precedence: env then config then default」）。驗證:cargo build -p fleety-server -p fleety-daemon 綠;「env 已設→不被覆寫」「env 未設+config 有值→採用」的邏輯單元測試（在 1.1 的 resolver/seed 上）。

## 4. 文件

- [x] 4.1 [P] docs/env.md：說明每個 FLEETY_* 設定同時是 config key、config.toml 位置與 scope 分區、precedence（env → config → default）、secret 遮罩——交付:文件與行為一致、與 registry 對齊。驗證:內容審查。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄互動 TUI 操作需手動驗證。
