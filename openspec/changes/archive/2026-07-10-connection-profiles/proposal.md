## Why

裝置端 CLI 連哪台 server 的設定散在三處、有優先序陷阱:`~/.fleety/config.json` 的 `agent_url`(`fleety init` 寫)、`config.toml` 的 `FLEETY_AGENT_URL`(registry Daemon scope、開機 seed 進 env)、env `FLEETY_AGENT_URL`。解析優先序讓 `config set FLEETY_AGENT_URL` 靜默蓋過 `fleety init`,使用者看不出為何 init 沒生效。`FLEETY_AGENT_URL` 被標成 Daemon scope 概念錯亂(它是 CLI 的連線目標),而 Cli scope 幾乎空。且沒有管理多台 server、切換連哪台的能力。完整脈絡見 docs/design-cli-config.md。這是 CLI 設定重新設計 Phase 1 的地基,provider/model、認證、互動面板都疊在它上面。

## What Changes

- 新增 `~/.fleety/connections.toml` 作為連線目標的唯一持久真相來源,與 fleety-tools 的共用連線模組(CLI 與 daemon 共用),檔案權限 0600。
- 新增 `fleety server` 命令群管理多台具名 server profile:add / use / list / show / current / rename / remove / set-url。`fleety init` 變成 `server add --use` 的 sugar,`fleety pair` 對當前 profile 配對並把 token 寫回該 profile。
- CLI 與 daemon 改用同一個連線 resolver(取代現行各自的 URL 解析),解析優先序單一化:`-s/--server`/`--url` 單次覆寫 > env `FLEETY_AGENT_URL`(臨時、永不寫檔)> `connections.toml` 的 current profile > mDNS(enrolled 後 sticky) > localhost。
- 從 config registry 移除 `FLEETY_AGENT_URL`,消除三處存的優先序陷阱。
- `config.json` → `connections.toml` 一次性、冪等、有備份的遷移。

## Non-Goals

- provider/model 兩層資料模型、認證改成預設開、互動全包面板、遠端互動 edit protocol——各自另開 change(見 docs/design-cli-config.md Phase 1/2 拆分)。
- 多裝置身分模型 / RBAC / 主人-一般裝置分級——post-v0。
- 連線憑證加密儲存 / OS keychain——本 change 只做 0600 檔案權限與 fingerprint 欄位保留,加密是後續強化。

## Capabilities

### New Capabilities

- `connection-profiles`: 具名 server 連線 profile 的資料模型、CLI 管理命令、CLI+daemon 共用解析器,與 config.json 的一次性遷移。

### Modified Capabilities

- `device-enrollment`: `fleety init` / `fleety pair` 改為對 connection profile 操作(寫 `connections.toml` 的 profile,而非 `config.json` 的扁平欄位);device_id 遷移時以既有值鎖定。
- `service-discovery`: mDNS 在新解析器中的位置(排在 current profile 之後)、enrolled 後 sticky pin、不把 profile token 送給 fingerprint 不符的 mDNS 解析 URL。

## Impact

- Affected specs: connection-profiles(新)、device-enrollment(改)、service-discovery(改)
- Affected code:
  - New: crates/fleety-tools/src/connection.rs
  - Modified: crates/fleety-cli/src/main.rs, crates/fleety-daemon/src/main.rs, crates/fleety-tools/src/config.rs, crates/fleety-cli/tests/cli_smoke.rs, crates/fleety-tools/src/lib.rs
  - Removed: (無檔案刪除;config.json 的讀寫路徑退場,舊檔改名備份保留)
