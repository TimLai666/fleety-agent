## Why

已配對裝置存的是 server 的固定 URL（sticky，刻意不漂移到 LAN 廣播者），所以 server 換了 IP（換網路、DHCP 重配）之後，裝置只會反覆重試舊位址，使用者必須手動重跑 init 或 set-url。使用者要的是「server IP 變了也不用做任何事」，但**不能跟其他台 server 搞混**。指紋機制的架子已預留（profile 有 fingerprint 欄位、resolver 有指紋守門），可是 server 沒有持久身份、mDNS 廣播沒帶指紋、配對時也沒 pin——整條鏈是空的，mDNS 發現的 server 因此永遠拿不到已存 token（安全但無法自癒）。

## What Changes

- server 產生**持久身份 id**（首次啟動生成、存於 agent home，跨重啟與 IP 變動不變），並在兩處曝露：mDNS TXT 屬性（供掃描比對）與 `Welcome` 的新增可選欄位（供已認證連線 pin 與校正）。
- 配對成功（Welcome 發 token）時，CLI 與 fleetyd 把 server 指紋 pin 進 current profile；**已配對的存量裝置**在下一次成功認證連線時自動補 pin（信任已認證連線上取得的指紋），profile 已有不同指紋時警告且不覆寫。
- CLI 的收集式掃描解析 TXT 指紋（Discovered 帶 fingerprint）；引導式 init 選單維持行為不變（選定配對時順帶 pin）。
- **黏著自癒**：CLI 與 fleetyd 對 current profile URL 連線失敗、且 profile 有 pinned 指紋、且 mDNS 未禁用時，做一次短掃描，**僅**當發現的廣播者指紋與 pinned 完全相符才把 profile 的 URL 更新為新位址並重連（持久化＋提示「server moved」）；指紋不符或缺席的廣播者一律忽略（token 絕不外流），掃無結果維持原錯誤。fleetyd 在重連退避迴圈內同樣自癒。
- docs/env.md 更新（指紋、癒合行為、信任邊界說明）。

## Non-Goals

- 不做挑戰-回應式的 server 身份證明：指紋是明文廣播的識別子，防的是「混淆與意外」，不防主動假冒——在現行 ws:// 明文 LAN 的信任邊界下，主動攻擊者本就能竊聽 token，指紋比對不劣化現狀；TLS/挑戰式驗證是既有的獨立後續。
- 不改變 sticky 原則本身：URL 能連上時永遠不掃描，成功路徑零開銷。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-discovery`: mDNS TXT 新增 server 身份指紋；收集式與單台發現攜帶指紋。
- `connection-profiles`: 指紋在配對與已認證連線時 pin 進 profile；連線失敗時的指紋比對自癒（僅相符者可更新 URL），token 永不交給指紋不符的廣播者。

## Impact

- Affected specs: `service-discovery`、`connection-profiles`
- Affected code:
  - Modified:
    - crates/fleety-server/src/mdns.rs
    - crates/fleety-server/src/main.rs
    - crates/fleety-server/src/conn.rs
    - crates/fleety-protocol/src/lib.rs
    - crates/fleety-tools/src/connection.rs
    - crates/fleety-cli/src/main.rs
    - crates/fleety-daemon/src/main.rs
    - docs/env.md
  - New: （無）
  - Removed: （無）
- 相容性：Welcome 欄位 additive（舊 client 忽略）；TXT 屬性 additive；無指紋的舊 server 面前，新 client 行為與現狀完全一致（不自癒、不外流 token）。
- 安全：token 只在指紋完全相符時才隨自癒重連送出；信任邊界（明文 LAN）不變並文件化。
