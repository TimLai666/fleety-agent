## Why

兩個第一次上手的體驗缺口。其一,pairing code 只能從 server 首啟 console、對話裡的 `pair_create` 工具、或 bootstrap token 取得——沒有 server 主機上的命令能直接生一組配對碼給其他裝置用。其二,`fleety init` 連上但沒配對時,`fleety ask` 會被 auth 擋下(正確),但 `fleety tui` 收到 server 的 `unauthenticated` 拒絕後,把它當成一般 turn 錯誤、連線關閉又當成暫時斷線,於是無限重連,使用者完全不知道問題是「還沒配對」。

## What Changes

- **`fleety pair-code` 命令**:連上目前指向的 server(本機靠 loopback 信任、遠端靠已配對 token),請 server 鑄造一組短效配對碼並印出——讓你在 server 主機上一行生碼,拿去別的裝置 `fleety pair <code>`。新增 `MintPairingCode` 請求 frame 與回覆;server 僅在認證開啟時鑄造(認證關閉時配對碼無意義,回明確說明)。能到達鑄造點的連線必已通過 Hello 認證(有效 token 或 loopback 信任),隨機未認證的 LAN 連線到不了這裡。
- **TUI 認出認證拒絕不再空轉**:TUI 收到 `unauthenticated` 的 Error 時,視為終止狀態——顯示可讀訊息(尚未與此 server 配對,請 `fleety pair <code>`,可用 `fleety pair-code` 生碼)並結束,不再無限重連。其他 Error 與暫時斷線的重連行為不變。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `device-enrollment`: 新增經連線鑄造配對碼——`MintPairingCode` frame 與 `fleety pair-code` 命令(認證開啟時鑄造、對已認證/loopback 信任連線放行)。
- `interactive-chat-tui`: TUI 對 `unauthenticated` 拒絕以可讀訊息終止,不再把認證失敗當暫時斷線無限重連。

## Impact

- Affected specs: `device-enrollment`、`interactive-chat-tui`
- Affected code:
  - Modified:
    - crates/fleety-protocol/src/lib.rs
    - crates/fleety-server/src/conn.rs
    - crates/fleety-cli/src/main.rs
  - New: （無）
  - Removed: （無）
- 相容性:新 frame additive;`fleety pair-code` 對舊 server 收到 unsupported 回覆時給明確版本提示。TUI 只多辨識一種既有 Error kind,其他行為不變。
- 安全:配對碼只在認證開啟時鑄造;能到達 session 的連線必為已認證或 loopback 信任,隨機 LAN 未認證連線在 Hello 就被拒、到不了鑄造點。
