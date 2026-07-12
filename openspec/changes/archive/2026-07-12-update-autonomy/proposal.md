## Why

更新面有三個讓 fleet 無法自主保持最新的缺口。其一，server-only 主機沒有更新指令：`fleety update` 是 CLI 指令，但 install-server.sh 只裝 server 與 sidecar，`fleety-server` 自己沒有 update 動詞——結果「在 server 主機跑一次更新、全 fleet 收斂跟上」的模型在最關鍵的那台機器上反而無路可走，只能重跑安裝腳本。其二，daemon 的 24 小時輪詢預設只通知（`FLEETY_AUTO_UPDATE` 預設 notify），使用者要的是無人值守自動更新——裝置預設就該自己裝新版。其三，`fleetyd update` 與輪詢 apply 只更新 fleetyd 自己＋sidecar，同機的 fleety CLI 與 fleety-server 不會跟著走（只有 server 版本收斂路徑會帶 sibling），同一台機器上的元件因此版本分裂。

## What Changes

- `fleety-server` 新增 `update` 動詞：以更新 manifest 自我更新（沿 `{bin}` 模板解析）、刷新 fleety-insyra sidecar、成功換檔後觸發既有的 deferred restart（等 idle）；install-server.sh 尾段提示補上這個指令。
- `FLEETY_AUTO_UPDATE` 預設值從 `notify` 改為 `apply`（**BREAKING**——行為預設反轉）：daemon 的 24 小時輪詢預設直接安裝新版；設 `notify`（或 `0`）退回僅通知。docs 同步。
- `fleetyd update` 與輪詢的 apply tick 改為 **host-wide**：更新 fleetyd 自己與 sidecar 之後，若 manifest 模板含 `{bin}`，同機的 `fleety` 與 `fleety-server`（存在者）一併更新到 latest，fleety-server 更新後以 bare restart 觸發 deferred 重啟；模板缺 `{bin}` 時跳過 sibling 並提示（沿既有守門，絕不以錯誤 binary 覆寫）。host-wide sibling 更新邏輯抽為共用（CLI 的 `fleety update` 與 daemon 路徑共用一份）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `self-update`: 輪詢預設改為 apply（MODIFIED）；新增 server 的 update 動詞要求（ADDED）；新增 daemon 更新攜帶同機 sibling 的要求（ADDED）。

## Impact

- Affected specs: `self-update`
- Affected code:
  - Modified:
    - crates/fleety-server/src/main.rs
    - crates/fleety-server/src/service.rs
    - crates/fleety-daemon/src/main.rs
    - crates/fleety-daemon/src/poll_updates.rs
    - crates/fleety-tools/src/update.rs
    - scripts/install-server.sh
    - docs/env.md
    - README.md
  - New: （無）
  - Removed: （無）
- 相容性：`FLEETY_UPDATE_MANIFEST` 未設時一切照舊跳過（更新面整體仍是 opt-in——沒設 manifest 什麼都不會發生，所以預設 apply 只影響已啟用更新的裝置）；顯式設了 `notify` 的部署不受影響。
- 風險面：預設 apply 代表裝置會在無人確認下換 binary——下載經 sha256 驗證、swap 具回滾、重啟等 idle，且 forward-only。
