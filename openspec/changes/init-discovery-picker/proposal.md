## Why

CLI 剛裝好的第一次上手體驗有缺口：mDNS 自動發現是**隱形的**——resolver 掃到 LAN 上第一台 server 就直接用，使用者看不到發現了什麼、也不能在多台 server 之間選；而顯式路徑 `fleety init` 又要求手打 `ws://host:8787`。server 端明明已在 mDNS 廣播帶有可讀名稱（`fleety-<hostname>`），CLI 卻沒有「掃描 → 列出 → 選一台 → 配對」的引導流程。使用者要的第一次體驗是：裝好 CLI、打一個指令、從清單選 server、輸入配對碼、完成。

## What Changes

- `fleety init` **不帶 URL** 且 stdout 是 TTY 時，改為引導式上手：掃描 LAN 收集**所有**廣播中的 fleety server（收集期內去重），以編號清單顯示（廣播的 instance 名稱＋ws URL，已存在於 connections.toml 的 profile 加標記），使用者輸入編號選擇。
- 選定後自動建立／更新 profile 並設為 current（profile 名預設取 instance 名去掉 `fleety-` 前綴，`--name` 可覆蓋），接著提示輸入配對碼（可留空跳過）：有輸入即走既有 pair 流程，成功後印出「已配對並設為目前 server」。
- mDNS 發現新增多台收集函式（固定收集窗口、以 URL 去重、帶 instance 名稱），現行單台 fallback 解析（resolver 內的隱式 mDNS）行為不變。
- 掃描無結果、非 TTY、或 `FLEETY_MDNS_DISABLED` 時，維持現行 usage 提示（含 ws:// 範例），不進入互動流程；`fleety init <ws-url>` 顯式用法完全不變。
- docs/env.md 與 README 的上手說明更新為新的第一次流程。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-discovery`: 新增互動式多台收集——在固定窗口內瀏覽並回傳所有發現的 server（instance 名稱＋URL，去重），供上手選單使用；隱式單台 fallback 不變。
- `device-enrollment`: `fleety init` 無 URL 的 TTY 情境改為「掃描 → 選擇 → 建 profile → 引導配對」的第一次上手流程；顯式 URL 用法與非 TTY 行為不變。

## Impact

- Affected specs: `service-discovery`、`device-enrollment`
- Affected code:
  - Modified:
    - crates/fleety-cli/src/main.rs
    - docs/env.md
    - README.md
  - New: （無）
  - Removed: （無）
- 相容性：`fleety init <ws-url>`、`fleety pair <code>`、resolver 的隱式 mDNS fallback 全部不變；新流程只接管「無參數＋TTY」這個目前必然失敗（usage 錯誤）的入口。
- 安全：選單只做發現與顯示；配對仍走既有 pairing code 驗證，token 寫入沿既有 profile 機制。
