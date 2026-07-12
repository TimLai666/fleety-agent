## Why

「本機 server 一級化」這輪只做進了 `fleety init` 引導選單,兩個平行面漏掉了(正是 AGENTS.md 新規則要防的不一致):(1) 三區設定面板的 Connection 區只列 connections.toml 既有 profile,不探測本機 server——從沒 init 過的 server 主機在面板裡選不到、也加不了本機;(2) 所有走 `connect_hello` 的一次性指令(pair-code / status / audit / rollback / conversations),對未配對的 `unauthenticated` 拒絕回的是 `expected welcome, got Some(Error { … })` 的 Debug dump,不是可讀的「你還沒配對」——TUI 這輪已修成友善訊息,一次性指令還沒比照。

## What Changes

- **設定面板 Connection 區探測本機 server**:開面板時以短逾時探測本機 server,若它有回應且尚無 profile 指向它,就在(記憶體中的)連線清單頂端加一個 `local` 條目;使用者按既有的 `u` 設為 current、`s` 存檔即建立 `local` profile 並切過去(loopback 信任,免配對)。與 `fleety init` 的本機優先行為對齊。
- **`connect_hello` 給可讀的認證錯誤**:`connect_hello` 收到 `unauthenticated` 的 Error 時,回可讀訊息(尚未與此 server 配對,請 `fleety pair <code>`,可用 `fleety pair-code` 在 server 主機生碼),不再回 Debug dump;其他 Error 以 server 訊息呈現、其他非預期 frame 給一般可讀訊息。所有走 connect_hello 的一次性指令一併受惠。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `interactive-config-panel`: 面板 Connection 區探測並列出本機 server 作為可選、可設為 current 的條目(免配對,靠 loopback 信任)。
- `device-enrollment`: 一次性指令的連線握手(`connect_hello`)對認證拒絕給可讀訊息,不印內部型別的 Debug 形式。

## Impact

- Affected specs: `interactive-config-panel`、`device-enrollment`
- Affected code:
  - Modified:
    - crates/fleety-cli/src/main.rs
    - crates/fleety-cli/src/config_panel.rs
  - New: （無）
  - Removed: （無）
- 相容性:純 CLI 行為擴充,無 wire/protocol 變更。面板無本機 server 時行為不變;connect_hello 對非認證錯誤與成功握手行為不變。
- 安全:面板選本機仍走 loopback 信任(同機才免配對);錯誤訊息不外洩敏感資訊。
