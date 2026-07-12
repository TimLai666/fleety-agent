## Context

現成積木:

- `crates/fleety-protocol/src/lib.rs` 的 `ServerMsg::Welcome` 已帶 `server_version`(server 的 `agent_core::VERSION`,additive、舊 server 為 `""`)。
- `crates/fleety-tools/src/update.rs`:`is_newer(a,b)`、`converge_self_to_version(version)->Result<bool>`(true=已換新 binary;forward-only;透過 latest manifest 的 `versioned_manifest` 欄位 pin 到確切版本;swap_exe 已修為 chmod 0o755)。
- `crates/fleety-daemon/src/main.rs` 的 `converge_to_server_version` 是 daemon 版範式:server 較新 → converge_self_to_version → `request_self_restart`(交給服務管理器重啟)。CLI 沒有服務管理器,改用 re-exec。
- CLI 在多處收 Welcome(`connect_hello`、`connect_hello_for_auth`,及各指令 inline),`agent_core::VERSION` 是本機版本。

## Goals / Non-Goals

**Goals:** CLI 連上較新 server 時自動 forward-only 收斂到 server 版本並 re-exec 當前指令;預設開、可關、graceful、不會迴圈。

**Non-Goals:** 不改 daemon 收斂、server 端、版本比較/manifest 機制;不做降版;不 push binary。

## Decisions

### 決策一:共用收斂 hook maybe_converge_cli(server_version)

在 `crates/fleety-cli/src/main.rs` 新增 `async fn maybe_converge_cli(server_version: &str)`:

1. 若 `server_version` 為空、或 `FLEETY_CLI_AUTO_UPDATE` 關、或 guard env `FLEETY_CONVERGED` 已設 → 直接 return(不動作)。
2. 若非 `is_newer(server_version, agent_core::VERSION)`(server 未較新)→ return。
3. 否則印一行「updating fleety <me> → <server_version> to match the server…」,呼叫 `fleety_tools::update::converge_self_to_version(server_version)`:
   - `Ok(true)`(已換新 binary)→ **re-exec**(見決策二),不返回。
   - `Ok(false)`(無對應/未換)或 `Err(_)` → 印可讀警告(含 server 版本與「以現行版本繼續」),return,讓當前指令照舊跑。

判斷邏輯(gate、is_newer 決策)抽為純函式 `should_converge(server_version, me, enabled, already_converged)->bool` 便於單元測試。

### 決策二:跨平台 re-exec + 防迴圈 guard

`converge_self_to_version` 換掉的是**磁碟上的 binary**,當前記憶體仍是舊碼,故要以新 binary 重跑當前指令:

- 取 `std::env::current_exe()` 與 `std::env::args_os()`(跳過 argv[0]),組同參數的新行程。
- 先設 `FLEETY_CONVERGED=1`(guard),讓 re-exec 後的行程略過收斂(即使因某種原因新版仍舊比 server 舊,也只嘗試一次、不無限迴圈)。
- Unix:`std::os::unix::process::CommandExt::exec()`(以新映像替換當前行程;失敗才返回)。
- Windows:`Command::spawn` 新行程 → `wait` → 以其 exit code `std::process::exit`。
- re-exec 前的清理:當前這條連線會被丟棄(re-exec 會重連),可接受(僅在罕見的版本不符時發生)。

### 決策三:接進收 Welcome 的路徑

在 CLI 收到 Welcome、取用其他欄位之前呼叫 `maybe_converge_cli(&server_version).await`。優先接在共用輔助 `connect_hello` / `connect_hello_for_auth`,並補上主要指令(ask / tui / pair-code / resume)inline 收 Welcome 之處,確保「任何連上較新 server 的指令」都會收斂。既有以 `..` 忽略 Welcome 欄位的 match 需改為擷取 `server_version`。

### 決策四:設定

`crates/fleety-tools/src/config.rs` registry 新增 `FLEETY_CLI_AUTO_UPDATE`(scope Cli、on/off、`v_onoff`、預設 on),同步 `setting_choices`。gate 讀原始 env(`!= off/0`)以與其他 on/off 慣例一致。

## Implementation Contract

**Behavior:**

- CLI 連上 server、Welcome 帶較新 `server_version`、`FLEETY_CLI_AUTO_UPDATE` 未關、非 re-exec 後 → 自動把 CLI binary 更到 server 版本 → 以相同 argv re-exec,當前指令跑在對齊版本上。
- server 同版或較舊 → 不動作。收斂失敗 → 警告 + 以現行版本繼續。
- 關閉(`FLEETY_CLI_AUTO_UPDATE=0`)→ 完全不收斂。

**Interface / data shape:**

- `async fn maybe_converge_cli(server_version: &str)`;純函式 `should_converge(server_version:&str, me:&str, enabled:bool, already:bool)->bool`;跨平台 `fn reexec_current() -> !`(或回 Result 後 exit)。
- 新 env `FLEETY_CLI_AUTO_UPDATE`(on/off,進 registry + config list);一次性 guard `FLEETY_CONVERGED`。

**Failure modes:**

- converge Err / Ok(false) → 警告、不 re-exec、不阻斷。
- re-exec spawn 失敗(Windows)或 exec 失敗(Unix)→ 警告、以現行版本繼續。
- 空 server_version(舊 server)→ 不動作。

**Acceptance criteria:**

- 單元測試:`should_converge` 真值表 —— server 較新+開+未收斂→true;server 同版/較舊→false;關閉→false;guard 已設→false;空 server_version→false。
- 手動/整合:舊 CLI 連新 server → 更新後 re-exec、以新版跑;`FLEETY_CLI_AUTO_UPDATE=0` → 不更新。
- `cargo test --workspace`、`clippy -D warnings`、`fmt --check` 乾淨。

**Scope boundaries:**

- In:CLI 收 Welcome 後的收斂 + re-exec、`FLEETY_CLI_AUTO_UPDATE`、docs、self-update spec。
- Out:daemon 收斂、server 端、版本/manifest 機制、降版。

## Risks / Trade-offs

- **re-exec 打斷當前連線**:僅在罕見的版本不符時發生,可接受;guard env 防迴圈。
- **自動更新的意外性**:使用者已選「預設開、可關」;提供 `FLEETY_CLI_AUTO_UPDATE=0`。
- **權限**:CLI binary 需可寫才能自我更新;失敗則 graceful 警告(不阻斷)。swap_exe 已修 chmod。
- **多接點**:Welcome 在多處被讀,需逐一接上;以 `should_converge` 純函式 + 共用 `maybe_converge_cli` 降低重複與遺漏。
