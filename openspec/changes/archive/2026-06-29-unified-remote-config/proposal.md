## Why

CLI 已是操控整個 fleet 的控制面(status 查 server、daemon <verb> 轉發 fleetyd、pair),唯獨 **設定** 還停在「每台主機改自己的本機檔」:`fleety config`(含 provider/group/role)只寫本機 ~/.fleety,沒有走網路,server 端也沒有任何遠端改 config 的訊息。結果是要設遠端 server 的模型,得 SSH 到 server 上跑 `fleety-server config`——跟「一套 CLI 同時管 server + client」(openclaw 風格)的設計初衷不符。本案補上核心缺口:**從 CLI 經連線管理 server 的設定**(含模型 / provider 池),把 server 設定面接上 CLI。裝置端設定(routing 到 daemon)拆成後續 change。

## What Changes

- **協定**:fleety-protocol 新增 `ConfigList` / `ConfigGet` / `ConfigSet` / `ConfigUnset`(ClientMsg,帶 `target` 與 scope/key/value/provider 動詞等欄位,涵蓋扁平鍵與 provider/group/role)+ `ConfigResult`(ServerMsg,帶結果或 WireError)。照搬既有 device 定址 request/result 模式(對齊 `AuditList`/`RollbackList`/`ServerStatus`)。
- **target 定址(本案 MVP)**:預設 `server`(連到的那台);`local` 改 CLI 自己的 ~/.fleety。`<device-id>`(路由到 daemon)留作後續 change `remote-config-devices`——裝置路由要動 daemon、重用 RunTool dispatch + waiter,屬第四個子系統,拆出去保持本案聚焦。
- **handler**:server 重用既有 `fleety_tools::config`(list/get/set/unset + provider/group/role),在 server host 的 config_path/providers_path 上執行;不把 config 邏輯複製進協定層。
- **認證**:config 變更走已認證連線(`Hello` token + `FLEETY_REQUIRE_AUTH` + pairing),每次變更寫 audit;不另立 admin 角色。
- **套用語意**:server 寫檔後下一回合重建 ProviderTiers 熱套用;無法熱套用的鍵(如 `FLEETY_ADDR`)由 `ConfigResult` 回「需重啟才生效」。
- **本機後備保留**:`fleety-server config` / `fleetyd config` 仍在(首次開機 bootstrap、CLI 還連不上時)。

## Non-Goals

(本變更會建立 design.md,Non-Goals / 後續寫在 design 的 Goals/Non-Goals 與 Open Questions。)

## Capabilities

### New Capabilities

- `unified-remote-config`: 經連線、認證後的遠端設定管理——CLI 用 `--target server`(預設)對連到的 server 做 list/get/set/unset(含 provider/group/role),`--target local` 改 CLI 本機;server handler 重用既有 config 函式並套用/回報 reload 邊界;`fleety-server config` 保留為 bootstrap 後備。裝置端(`--target <device-id>`)為後續 change。

### Modified Capabilities

(none)

## Impact

- Affected specs: unified-remote-config(新)
- Affected code:
  - Modified:
    - crates/fleety-protocol/src/lib.rs(新增 Config* ClientMsg / ConfigResult ServerMsg + target 型別)
    - crates/fleety-server/src/conn.rs(Config* handler:認證閘 + 在 server host 執行 fleety_tools::config + 套用/reload)
    - crates/fleety-cli/src/config.rs(config 指令加 --target;預設送 server、target=local 維持寫本機;Config* 收送)
    - crates/fleety-cli/src/main.rs(config 子命令解析 --target、連線送訊息)
    - docs/env.md(遠端 config 用法 + --target 說明)
    - README.md(更新「Connecting & configuring」:CLI 可遠端管 server/device 設定)
