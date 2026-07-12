## Why

裝置上的 daemon(fleetyd)連上 server 時,已經會讀 Welcome 的 `server_version` 做 forward-only 收斂(`converge_to_server_version` → `converge_self_to_version`),把自己 pin 到 server 的確切版本。但**一般 CLI**(`fleety ask` / `tui` / `pair-code` / `resume` …)沒有這個機制:從另一台裝置用較舊的 CLI 連進較新的 server,CLI 就一直是舊版,行為/協定可能對不上,使用者得手動 `fleety update`。使用者要求:CLI 連上較新的 server 時**自動跟到 server 的版本**。機制大多現成(Welcome 已帶 server_version、convergence 函式已存在、swap_exe 的執行權限 bug 也已於 v0.1.12 修好),只需接到 CLI 的連線路徑。

## What Changes

- CLI 連上並收到 Welcome 後,讀 `server_version`;若 `is_newer(server_version, 本機版本)`(**forward-only**,永不降版),就呼叫既有的 `converge_self_to_version(server_version)` 把 CLI binary pin 到 server 的確切版本(透過 latest manifest 的 `versioned_manifest` 欄位,配合內建預設 manifest、免設環境變數)。
- 更新成功後,**以相同 argv re-exec 當前指令**,讓這一次就跑在對齊 server 的版本上。設一次性 guard 環境變數(如 `FLEETY_CONVERGED=1`)給 re-exec 後的行程,確保只收斂一次、不會無限迴圈。
- 預設開啟;`FLEETY_CLI_AUTO_UPDATE=0`(或 off)關閉。收斂失敗(權限/網路/manifest 無對應)→ 印可讀警告、以現行版本繼續,絕不阻斷指令。server 較舊或同版 → 不動作。
- 接點:CLI 收 Welcome 的共用路徑(`connect_hello` / `connect_hello_for_auth` 等共用連線輔助,及主要指令的 Welcome 接收處),抽一個共用 `maybe_converge_cli(server_version)` 呼叫。

## Non-Goals (optional)

- 不改 daemon 既有收斂(它維持自己的 opt-in gate);不改 server 端(server 已在 Welcome 帶 `server_version`,無需改動)。
- 不做 server 主動 push binary 到 CLI(server 無法推;而是 CLI 依 Welcome 的版本自我收斂)。
- 不改版本比較 / manifest / sha256 / versioned_manifest 機制(沿用既有)。
- 不做「CLI 比 server 新就把 server 降版」(forward-only)。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `self-update`: 新增「CLI 連上較新的 server 時 forward-only 收斂並 re-exec」的需求(對齊既有 daemon 收斂,但適用互動式 CLI;預設開、可 `FLEETY_CLI_AUTO_UPDATE` 關、graceful 降級、防迴圈 guard)。

## Impact

- Affected specs: `self-update`(modified)
- Affected code:
  - Modified:
    - crates/fleety-cli/src/main.rs — 新增 maybe_converge_cli(server_version)(呼叫 converge_self_to_version + 跨平台 re-exec + guard env)並接進收 Welcome 的共用路徑
    - crates/fleety-tools/src/config.rs — 新增 FLEETY_CLI_AUTO_UPDATE(on/off,預設 on)
    - docs/env.md — FLEETY_CLI_AUTO_UPDATE 文件
  - New: (none)
  - Removed: (none)
