## Why

Fleety 已能把裝置歸到 site(場域),但 `device.json` 的 `site` 只能手動設,永遠是靜態且過時的。使用者要的是「用裝置的所在位置推定人在不在某個場域、出門了沒、回來了沒」——這需要裝置能自動回報所在位置、系統記錄位置變化的時序,並以機率而非確定的方式表達推論。這是既有 sites / `mobility` / co-location 安全原則(「能連到 ≠ 人在場」)的自然延伸,也是後續自動化(依存在觸發動作)的前置基礎。

## What Changes

- **daemon 自動回報 co-location 信號(主要信號)**:fleetyd 週期性計算目前所在區網的網路指紋(預設閘道 MAC + 子網,並選配 mDNS 探到的同區網 Fleety 裝置清單),透過既有連線回報給 server。
- **server 依「指紋 → site」規則自動更新裝置 current site**:server 維護每個 site 的已知網路指紋;收到裝置回報時,若指紋比對到某 site,就自動把該裝置 `device.json` 的 `site` 更新為該 site(取代目前純手動)。未知指紋維持 `unknown`,待使用者/agent 綁定。
- **新增 `home_site`(慣常場域)基準欄位**:與「目前所在 site」分開,作為「偏離慣常位置 = 可能出門」的判斷基準。可由工具明確設定。
- **presence timeline(存在時間線)**:server 把每次 site 變更事件(裝置、from/to site、時間戳、信號來源、信心度)append 進持久化的存在時間線,才能回答「何時出門/回來」這類時序問題。
- **site 層級 presence 查詢工具**:新增讓 agent 查「某 site 現在有哪些裝置在、推定有沒有人在、信心度多少」與「某裝置目前推定在哪」的工具,輸出一律附信心度與依據,絕不當成確定事實。
- **隱私 opt-in(預設關閉)**:presence 追蹤採 per-device opt-in,由環境變數/設定開關控制;未 opt-in 的裝置 daemon 不回報 co-location 信號、server 不記錄其 timeline。資料屬使用者、納入既有 retention/GC。
- **server LAN 掃描補強(次要信號)**:server 週期性以既有 mDNS 瀏覽自身區網、佐證哪些裝置與 server 同場域;僅涵蓋 server 自己的網段,作為 daemon 自報的補強而非取代。

## Capabilities

### New Capabilities

- `presence-inference`: 依裝置 co-location 信號自動維護 current site、記錄存在時間線、並以機率化信心度回答 site/裝置層級的存在推定,受 per-device opt-in 約束。

### Modified Capabilities

(none)

## Impact

- Affected specs: presence-inference(新增)
- Affected code:
  - New:
    - crates/fleety-server/src/presence.rs
    - crates/fleety-daemon/src/colocation.rs
  - Modified:
    - crates/fleety-protocol/src/lib.rs
    - crates/fleety-daemon/src/main.rs
    - crates/fleety-server/src/conn.rs
    - crates/fleety-server/src/sites.rs
    - crates/fleety-server/src/storage.rs
    - crates/fleety-server/src/main.rs
    - crates/fleety-tools/src/config.rs
    - prompts/memory.md
    - docs/env.md
  - Removed: (none)
- Dependencies: 沿用既有 mDNS(service-discovery)、既有連線協定與 auth;不新增外部 crate(網路指紋以既有系統介面/標準庫取得,取不到時降級為 unknown 並記錄,不崩潰)。
