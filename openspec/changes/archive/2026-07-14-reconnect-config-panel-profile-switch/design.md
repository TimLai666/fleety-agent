## Context

Settings 面板目前在 run_settings 入口解析 current profile、建立一次 WebSocket，並在整個畫面生命週期內持有同一組 sender/receiver。Connection 區的 u 僅改變 Panel.conns.current，s 僅持久化 connections.toml，因此 UI 與遠端連線身分可能分離。遠端 snapshot、revision 與 staged changes 都綁定原連線，不能跨 server 重用。

## Goals / Non-Goals

**Goals:**

- current profile 成功儲存後，同一面板立即切換到該 profile 所代表的 server。
- 新 server 的 Server 與目前裝置 Daemon snapshot 取代舊狀態。
- 切換期間不得把舊 server 的 revision、snapshot 或 staged changes 送到新 server。
- 任一步失敗後都不得繼續使用舊連線。

**Non-Goals:**

- 不改變命令列 -s/--server 的一次性 override 規則。
- 不改變 connections.toml 格式或 server protocol。
- 不保留跨 server 的 staged edits，也不加入 staged edit migration。
- 不讓 Connection 區未儲存的 current 選擇觸發遠端切換。

## Decisions

### 儲存 current profile 後以明確 profile 建立新連線

儲存成功是切換交易的提交點。重連函式接收已選定的 profile 名稱或其解析結果，使用該 profile 的 URL、token 與 fingerprint，不再重新讀取可能於 async 等待期間改變的全域 current。替代方案是在每次 apply 前重新解析 current，但這會讓 snapshot 與 apply 不一定屬於同一台 server。

### 切換前使舊遠端狀態失效

開始重連時先取走並關閉舊 sender，清空 Server/Daemon 的 availability、revision、entries 與 staged changes，再嘗試新連線。這使任何重連失敗都只能留下 unavailable 狀態。替代方案是等新連線成功後才替換，會在等待或錯誤路徑保留可誤用的 B 狀態。

### 新連線分別刷新兩個 owner snapshot

Welcome 成功後，以同一條新 server 連線依序請求 ConfigTarget::Server 與 ConfigTarget::Device(current device id)。每個 snapshot 各自決定其區域 availability。Server snapshot 成功不代表 Daemon 必然可用。既有 structured-config capability gate 繼續適用。

## Implementation Contract

**Observable behavior**

- 使用者在 Connection 區將 current 從 B 指向 A 並按 s，儲存成功後狀態列顯示正在重連，接著顯示已連到 A 或可行動的失敗訊息。
- 成功後 Server 區只顯示 A 的 snapshot，Daemon 區只顯示 A 上目前 device id 的 daemon snapshot。
- B 的 staged changes、revision 與 entries 在切換開始時消失。下一次 apply 只能經由 A 的 sender。
- 若 A 無法連線、Hello 失敗或 frame malformed，Server 與 Daemon 都 unavailable，Connection 與 CLI 仍可操作，舊 B sender 已關閉。
- 若 A 連線成功但 daemon snapshot 失敗，Server 可用而 Daemon unavailable。

**Interface and state**

- 不新增 protocol frame 或設定格式。
- config panel run loop 增加一個 profile-switch intent，並集中由 async helper 建立連線與載入 owner snapshots。
- 遠端連線及兩個 region state 必須一起替換，避免分散更新形成半新半舊狀態。

**Acceptance criteria**

- 單元測試先重現 current 改成 A 後仍使用 B sender 的失敗。
- 測試證明切換開始會清除兩區 staged、revision 與 entries。
- async 測試或可注入 connector 的測試證明成功切換後 apply 只到 A。
- 測試證明重連失敗後沒有可用 sender，兩區 unavailable，Connection/CLI state 保留。
- cargo test -p fleety-cli、cargo fmt --all -- --check、cargo clippy -p fleety-cli --all-targets -- -D warnings 通過。

**Scope boundaries**

- In scope：crates/fleety-cli/src/config_panel.rs 的 Settings profile 切換、重連、snapshot refresh 及測試。
- Out of scope：provider editor 跨畫面流程、非互動 config 命令、daemon/server protocol 及其他 binary。

## Risks / Trade-offs

- [使用者未儲存的 staged edits 會在切換時遺失] → 切換狀態明確說明已丟棄舊 server staged changes。跨 owner 保留本來就不安全。
- [A 可連線但 daemon 未註冊] → Server 與 Daemon availability 獨立更新，不把局部失敗擴大成整體失敗。
- [重連時 UI 暫時等待網路] → 沿用既有連線 timeout 與錯誤回報，不新增無限等待。
