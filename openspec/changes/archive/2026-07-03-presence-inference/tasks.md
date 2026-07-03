## 1. Protocol 與資料模型基礎

- [x] 1.1 [P] 新增 additive frame `ClientMsg::Colocation { fingerprint: Option<String>, subnet: Option<String>, peers: Vec<String> }` 於 crates/fleety-protocol/src/lib.rs;PROTOCOL_VERSION 不變、舊端忽略。驗證:serde round-trip 單元測試 + 一個舊式訊息(無此 frame)反序列化不受影響的測試。
- [x] 1.2 [P] 在 device 記錄讀寫層(crates/fleety-server/src/storage.rs)支援新欄位 `home_site: String`(預設空)、`presence_opt_in: bool`(預設 false)、`site_since_secs: u64`;缺欄位讀為預設、既有 device.json 免遷移。驗證:對舊 device.json(無新欄位)讀出預設值、寫入後往返一致的單元測試。(涵蓋需求: Devices carry a home-site baseline distinct from current site)
- [x] 1.3 [P] 在 site 記錄支援 `fingerprints: [String]`(預設空)於 crates/fleety-server/src/sites.rs;缺欄位讀為空陣列。驗證:對既有 site json 讀出空 fingerprints、加入後往返一致的單元測試。

## 2. 存在推定核心(server presence.rs)

- [x] 2.1 建立 crates/fleety-server/src/presence.rs,實作純函式 `person_present_confidence(devices, mobility, home_sites) -> (f32, reasons)`:stationary 在場給弱信號、mobile 在其 home_site 給較強信號、mobile 離開 home_site 給出門傾向。驗證:對照 spec Example 表(stationary-only=low、mobile-at-home=higher、mobile-away=departure-leaning)的單元測試。(涵蓋需求: Presence is answered probabilistically with confidence and caveats)
- [x] 2.2 實作指紋→site 對映與 opt-in gating:已知指紋更新 current site、未知指紋維持 unknown 並標記待綁定、未 opt-in 一律不記錄。驗證:已知/未知/未 opt-in 三情境的單元測試。(涵蓋需求: The server maps fingerprints to sites and updates current site;Presence tracking is per-device opt-in and off by default)
- [x] 2.3 實作存在時間線 append 至 `fleet/presence/timeline.jsonl`,含去抖動(僅 current site 實際變更才寫一筆,同 site 不重複);寫入失敗記 warning 不崩潰。驗證:site 變更寫一筆、重複回報不新增、寫入錯誤不 panic 的單元測試。(涵蓋需求: The server records a presence timeline of site changes)

## 3. server 連線與工具接線

- [x] 3.1 在 crates/fleety-server/src/conn.rs 處理 `ClientMsg::Colocation`:路由到 presence 核心做 opt-in 檢查、指紋對映、site 更新與時間線寫入;無新增 ServerMsg。驗證:餵入已綁定指紋的 Colocation 後 device.json site 更新且時間線增一筆的單元測試。
- [x] 3.2 註冊五個 agent 工具 `presence_show`、`device_presence`、`site_bind_fingerprint`、`device_set_home_site`、`device_set_presence_opt_in`(回傳形狀見 design 的 Implementation Contract),對未知裝置/site/未綁定指紋回「原因+下一步」可行動錯誤。驗證:每個工具正常路徑 + 一個錯誤路徑的單元測試。
- [x] 3.3 把五個 presence 工具接進實際 registry(conn.rs 的連線工具組與 scheduler 工具組),與既有 site 工具並列。驗證:registry 建好後可 call 到這五個工具名的測試。

## 4. daemon 端 co-location 自報

- [x] 4.1 [P] 建立 crates/fleety-daemon/src/colocation.rs:跨平台計算網路指紋(預設閘道 MAC + 子網,雜湊儲存不留明文;Windows/Linux/macOS 各自從 OS 既有介面取得),取不到回 None。驗證:指紋為雜湊字串、取不到回 None 的單元測試(平台相關部分以可注入的取值函式包裝以便測試)。(涵蓋需求: Devices self-report co-location signals under opt-in)
- [x] 4.2 在 crates/fleety-daemon/src/main.rs 加週期性回報:`FLEETY_PRESENCE=on` 才計算並每 `FLEETY_PRESENCE_INTERVAL_SECS`(預設 300、下限鉗制)於連線/心跳邊界送 `Colocation`;off(預設)完全不送。驗證:off 不送、on 依週期送的邏輯以可測的 tick 函式驗證。

## 5. 次要佐證與設定

- [x] 5.1 server 同網佐證(修正:subnet 比對,非 mDNS——fleetyd 不宣告 mDNS,browse 找不到裝置)。daemon 的 Colocation 已帶 subnet,server 存每裝置 last_subnet 並算自身 subnet;裝置 subnet == server subnet 即同網段,pure function `corroborate` 據此加分並附 reason,接進 `presence_show`;明示僅涵蓋 server 網段。驗證:給定已佐證同網的裝置,confidence reasons 反映佐證的單元測試。
- [x] 5.2 [P] 在 typed config registry(crates/fleety-tools/src/config.rs)新增 `FLEETY_PRESENCE`(off|on,預設 off)與 `FLEETY_PRESENCE_INTERVAL_SECS`(預設 300、60 下限),含 scope 與預設。驗證:config list/get 顯示新鍵與預設值、未知值回退的單元測試。
- [x] 5.3 把 `fleet/presence/timeline.jsonl` 納入既有 GC 輪替(crates/fleety-server/src/gc.rs),超過大小上限旋轉。驗證:超過上限觸發旋轉、未達上限不動的單元測試。

## 6. 文件與端到端

- [x] 6.1 [P] 更新 prompts/memory.md:加入 presence 使用紀律——機率化、附信心度與依據、明示「可達≠在場」、謹慎揭露他人存在。驗證:內容審查,確認涵蓋 opt-in、信心度、caveat 三點。
- [x] 6.2 [P] 更新 docs/env.md(FLEETY_PRESENCE / FLEETY_PRESENCE_INTERVAL_SECS)與 docs/tools.md(五個新工具),與實作一致。驗證:內容審查對照工具名與環境變數。
- [x] 6.3 端到端 localhost 測試(比照既有 daemon↔server 測試):opt-in 的 daemon 送已綁定指紋的 Colocation → server 更新 device.json site + 寫一筆時間線 → `presence_show` 反映在場並帶 confidence。驗證:該整合測試通過。
