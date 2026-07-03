## Context

Fleety 已有 site(場域)模型:`fleet/sites/<id>.json` 記錄 site,`device.json` 帶手動設定的 `site` 與 `mobility` 欄位(見 crates/fleety-server/src/sites.rs 的 site_set / device_set_site / device_set_mobility)。缺口是這些欄位純手動、永遠過時,系統無從自動得知裝置移動到哪、也沒有時序記錄。既有安全原則「能連到 ≠ 人在場」要求存在推定必須機率化、且靠區網共處(co-location)而非可達性判斷同場域。既有 mDNS(service-discovery)提供區網探索、既有連線協定(fleety-protocol)提供 daemon↔server 通道、既有 retention/GC(gc.rs)可回收時序資料。本變更在這些基礎上補齊「自動現場偵測 + 基準 + 時間線 + 查詢」。

## Goals / Non-Goals

**Goals:**

- daemon 能自動算出目前所在區網的網路指紋並週期性回報,受 per-device opt-in 約束、預設關閉。
- server 能把已知指紋對映到 site、自動更新裝置 current site,未知指紋維持 unknown 並可被綁定。
- 每台裝置有與 current site 分離的 home_site 基準欄位。
- server 持久化存在時間線(site 變更事件含時間戳、信號來源、信心度)。
- agent 能查 site 層級與裝置層級的存在推定,輸出一律附信心度與依據、明示「可達 ≠ 在場」。
- server 以既有 mDNS 掃描自身區網作為次要佐證信號。

**Non-Goals:**

- 人員身分/生物特徵辨識、硬綁定「某裝置 = 某人」。
- GPS / 地理定位 API、行動裝置原生 app(桌機優先,spec 明確排除 mobile client)。
- home_site 的自動推斷(由時間線推最常出現 site);本次只做明確設定,自動推斷列為後續。
- 存在觸發的自動化/通知(依「有人回家」自動開燈之類);本次只產生存在資料,不觸發動作。
- 跨區網、超出 daemon 自報範圍的存在偵測;server 掃描僅涵蓋自身網段。
- ML 預測。

## Decisions

**D1 網路指紋組成 = 預設閘道 MAC + 子網 CIDR(主),選配 mDNS 同網 peer 集合。** 閘道 MAC 對單一實體區網穩定、換網即變、Wi-Fi 與有線皆適用,且可由 OS 既有介面取得(Windows 讀 ARP、Linux 讀 /proc/net/arp 或 ip neigh、macOS 讀 arp),不需新外部 crate。取不到時指紋為 None。捨棄的替代:SSID(僅 Wi-Fi)、對外 IP(會變且洩漏)、全網段掃描(慢且侵入)。

**D2 指紋 → site 綁定採「明確綁定 + 只在已知指紋時自動更新」。** server 在 site 記錄(sites/<id>.json)新增 `fingerprints: []`。收到裝置回報指紋時:若指紋已對映到某 site,自動把該裝置 `device.json` 的 `site` 更新為該 site;若指紋未知,設 site 為 `unknown` 並在回報中標明「未知指紋、待綁定」。綁定由工具 `site_bind_fingerprint`(把某裝置目前回報的指紋加入某 site)完成,不讓 daemon 自行斷言任意 site,避免誤學。

**D3 home_site 為明確欄位、明確設定。** `device.json` 新增 `home_site`,由工具 `device_set_home_site` 設定,與 current `site` 分離。自動推斷列為 Non-Goal。

**D4 存在時間線 = 中央 append-only JSONL。** 路徑 `fleet/presence/timeline.jsonl`;每筆事件 `{ ts_secs, device, from_site, to_site, signal, confidence }`,只在 server 判定裝置 current site 實際變更時 append(去抖動:同 site 重複回報不寫)。中央存放便於 site 層級聚合查詢。納入既有 GC 輪替(超過大小上限旋轉),沿用 FLEETY_HISTORY_ROTATE 類機制或等值。

**D5 信心度為機率分數 + 依據,永不布林。** site 層級聚合:stationary 裝置在該 site 代表「site 可達」,對「有人在」只給弱信號;mobile 裝置(手機/筆電)在其 home_site 是較強的「人可能在家」信號,mobile 裝置離開 home_site 是「人可能出門」信號。每個答案回 `confidence`(0..1)+ `reasons[]` + 固定 caveat「reachable ≠ present」。休眠/留置裝置會誤判,故需多信號、且信心度反映之。

**D6 opt-in 兩道(縱深):** daemon 端 `FLEETY_PRESENCE`(預設 off)控制是否計算/回報指紋——off 就完全不送;server 端 `device.json` 的 `presence_opt_in`(預設 false)控制是否記錄該裝置的 site 變更與時間線。兩者皆關則完全無追蹤;資料屬使用者、可經既有 config/工具檢視與關閉。

**D7 協定新增 additive frame `ClientMsg::Colocation`。** daemon 週期性(睡醒/換網/心跳邊界)送 `Colocation { fingerprint: Option<String>, subnet: Option<String>, peers: Vec<String> }`。用獨立 frame 而非塞進 Hello,因為 co-location 會隨時間變化(睡眠、換網)。additive、PROTOCOL_VERSION 不變、舊 server/client 忽略仍可運作(比照既有 ConversationRolled 的相容做法)。

**D8 server 同網佐證(修正:subnet 比對,非 mDNS 瀏覽)。** 實作時發現原設計「server 以 mDNS 瀏覽自身區網找裝置」不可行——fleetyd 不對 mDNS 宣告(只有 server 宣告、供 client 探索 server),browse 找不到裝置。改用已有資料:daemon 的 `Colocation` 已帶 `subnet`,server 可算出自身 subnet;裝置回報的 subnet 與 server 自身 subnet 相同即代表同網段,作為 confidence 的補強(pure function `corroborate`)。僅涵蓋 server 網段、不取代 daemon 自報。此為 apply 期間的設計修正。

## Implementation Contract

**行為(Behavior):** 在一台已 opt-in 的 fleetyd(`FLEETY_PRESENCE=on`)連上、且其目前區網指紋已綁定到 site `home` 時,server 會自動把該裝置 `device.json` 的 `site` 設為 `home`,並在時間線寫一筆 site 變更;agent 呼叫 presence 查詢工具會得到「該 site 目前的裝置清單 + 機率化的『是否有人在場』推定 + 信心度 + 依據 + caveat」。未 opt-in 的裝置完全不回報、不被記錄。

**介面 / 資料形狀(Interface / data shape):**

- 協定:`ClientMsg::Colocation { fingerprint: Option<String>, subnet: Option<String>, peers: Vec<String> }`;無新增 ServerMsg(server 靜默處理,更新落在既有 device 記錄與時間線)。
- `device.json` 新增欄位:`home_site: String`(預設空)、`presence_opt_in: bool`(預設 false)、`site_since_secs: u64`(current site 起始時間戳,可選)。既有 `site`/`mobility` 語意不變。
- `sites/<id>.json` 新增欄位:`fingerprints: [String]`(預設空陣列)。
- 時間線:`fleet/presence/timeline.jsonl`,每行 `{ ts_secs: u64, device: String, from_site: String, to_site: String, signal: String, confidence: f32 }`。
- 新增 agent 工具(名稱與回傳):
  - `presence_show { site }` → `{ site, devices: [{ device, mobility, present_signal }], person_present: { confidence: f32, reasons: [String] }, caveat }`。
  - `device_presence { device }` → `{ device, site, home_site, since_secs, confidence: f32, reasons: [String], caveat }`。
  - `site_bind_fingerprint { device, site }` → 把該裝置目前回報的指紋加入該 site 的 fingerprints,回 `{ site, fingerprint, bound: true }`;裝置無已知指紋時回可行動錯誤。
  - `device_set_home_site { device, home_site }` → 設 home_site(需為已註冊 site 或保留字),回 `{ device, home_site, set: true }`。
  - `device_set_presence_opt_in { device, enabled }` → 設 per-device opt-in,回 `{ device, presence_opt_in: bool, set: true }`。
- 設定:daemon `FLEETY_PRESENCE`(off|on,預設 off)、回報週期 `FLEETY_PRESENCE_INTERVAL_SECS`(預設 300、下限鉗制);皆入 typed config registry。

**失敗模式(Failure modes):** 指紋取不到 → `fingerprint: None`,server 視為 unknown、不變更 site、不崩潰。未知指紋 → site=unknown 且回報標明待綁定。未 opt-in → daemon 不送、server 收到也不記錄。時間線寫入失敗 → 記 warning、不影響連線與回合(永不崩潰原則)。所有工具對未知裝置/site/未綁定指紋回「原因 + 下一步」的可行動錯誤。

**驗收(Acceptance criteria):**

- 單元測試:指紋 → site 對映(已知/未知)、site 變更去抖動(同 site 不重複寫時間線)、opt-in 關閉時不記錄、confidence 純函式(給定裝置集合與 mobility/home_site → 分數與 reasons)、工具對錯誤輸入回可行動錯誤、`Colocation` frame 序列化往返。
- 端到端測試(比照既有 localhost 測試):opt-in 的 daemon 送 `Colocation`(指紋已綁定 site)→ server 更新 device.json site + 寫一筆時間線 → `presence_show` 反映該裝置在場、附 confidence。
- 相容:未帶 `Colocation` 的既有 client 行為不變;PROTOCOL_VERSION 不變。

**範圍邊界(Scope boundaries):** In scope = 上述協定/欄位/時間線/五個工具/兩道 opt-in/daemon 指紋計算與週期回報/server mDNS 佐證/prompt 與 docs 更新。Out of scope = 人員身分辨識、GPS/mobile app、home_site 自動推斷、存在觸發的自動化或通知、跨網段偵測、ML。

## Risks / Trade-offs

- [誤判:mobile 裝置留在家或休眠 → 誤推「人在家」] → confidence 反映之、多信號佐證、輸出永遠附 caveat 與 reasons,不當確定。
- [隱私:存在資料極敏感] → 兩道 opt-in 且預設全關、資料屬使用者、納入 GC、prompt 強調謹慎揭露;未 opt-in 完全不採集。
- [閘道 MAC 在某些網路(如企業 VLAN、多 AP 漫遊)不穩或相同] → 指紋為輔助信號,綁定採明確工具、未知即 unknown,不硬猜;peers 集合可作為額外佐證維度。
- [server 掃描只見自身網段] → 明示為次要佐證、不取代 daemon 自報;文件寫清楚。
- [指紋洩漏網路資訊] → 指紋以雜湊儲存(sites/device 只存 hash),不存原始 MAC 明文。

## Migration Plan

- 純新增:新欄位有預設值(home_site 空、presence_opt_in false、fingerprints 空),舊 `device.json`/`sites/*.json` 無需遷移即相容(缺欄位讀為預設)。
- 部署:預設全關(`FLEETY_PRESENCE` off、per-device opt-in false),不影響現有部署行為;使用者顯式開啟才生效。
- 回退:停用只需 `FLEETY_PRESENCE=off` 或關 per-device opt-in;時間線檔可刪,不影響其他子系統。

## Open Questions

- 指紋是否納入 mDNS peers 作為必要維度,或僅閘道即可?初版以閘道為主、peers 為選配佐證,實測後再決定加權。
- home_site 自動推斷(從時間線)是否值得後續獨立 change?視使用回饋。
