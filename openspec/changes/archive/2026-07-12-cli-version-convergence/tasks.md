## 1. CLI 收斂 hook 與 re-exec(crates/fleety-cli/src/main.rs)

- [x] 1.1 依 design「決策一:共用收斂 hook maybe_converge_cli(server_version)」「決策二:跨平台 re-exec + 防迴圈 guard」與 spec「The CLI converges to the server's version on connect」:新增純函式 should_converge(server_version, me, enabled, already)->bool(server 較新+開+未收斂→true;同版/較舊/關/已收斂/空→false)、async maybe_converge_cli(server_version)(true→converge_self_to_version→re-exec;false/Err→警告續跑)、跨平台 reexec_current(Unix exec / Windows spawn+wait+exit,先設 FLEETY_CONVERGED guard)。先寫測試(tdd):should_converge 真值表。驗證:cargo test -p fleety-cli should_converge 全綠。
- [x] 1.2 依 design「決策三:接進收 Welcome 的路徑」:在 connect_hello / connect_hello_for_auth 及主要指令(ask/tui/pair-code/resume)收 Welcome 處呼叫 maybe_converge_cli(&server_version)(把以 .. 忽略的 Welcome match 改為擷取 server_version)。驗證:cargo build -p fleety-cli 乾淨、既有 CLI 測試不回歸。

## 2. 設定與文件

- [x] 2.1 [P] 依 design「決策四:設定」:config.rs registry 新增 FLEETY_CLI_AUTO_UPDATE(scope Cli、on/off、v_onoff、預設 on)並同步 setting_choices()。驗證:cargo test -p fleety-tools 全綠、FLEETY_CLI_AUTO_UPDATE 出現在 config list。
- [x] 2.2 [P] docs/env.md 補 FLEETY_CLI_AUTO_UPDATE(連上較新 server 時自動 forward-only 收斂並 re-exec;預設 on;0/off 關)。驗證:內容審閱與 spec 用語一致。

## 3. 整體驗證

- [x] 3.1 全 workspace 驗證:cargo test --workspace、cargo clippy --workspace --all-targets -- -D warnings、cargo fmt --all -- --check 乾淨。驗證:指令輸出乾淨。
