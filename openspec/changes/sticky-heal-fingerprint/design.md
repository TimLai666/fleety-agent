## Context

連線黏著是刻意的：resolver 對 current profile 的 URL 永遠直用、不回退 mDNS（已配對裝置不漂移到 LAN 廣播者）。代價是 server 換 IP 後裝置只會重試舊位址。指紋防護的半成品：Profile 有 `fingerprint: Option<String>` 欄位、`Discovered` 有 fingerprint、resolver 的 mDNS 分支只把 token 交給指紋相符的發現者——但 server 端沒有持久身份（mDNS TXT 只帶 version）、CLI 端建 Discovered 時 fingerprint 恆為 None、配對流程也從不寫 profile.fingerprint。pair 成功的訊號是 `Welcome { token: Some(_) }`（CLI 的 pair 與 fleetyd 各自處理）。fleetyd 有指數退避重連迴圈；CLI 每個指令是一次性連線（open() 失敗即報錯）。mDNS 掃描已有單台（2 秒早退）與收集式（3 秒全collect，v0.1.3）兩種。

信任邊界現狀：wire 是 ws:// 明文，LAN 內竊聽者本就能看到 token 與流量——所以「明文廣播的指紋」不會劣化威脅模型（防混淆與意外，不防主動假冒；主動假冒者在現行邊界下有更直接的攻擊面）。TLS／挑戰式身份證明是既有的獨立後續項。

## Goals / Non-Goals

**Goals:**

- server 換 IP 後，已配對裝置（CLI 與 fleetyd）自動找回**同一台** server（指紋相符）、更新 profile URL、以原 token 重連——使用者零操作。
- 絕不與其他 server 搞混：指紋不符或缺席的廣播者一律忽略，token 不外流。
- 成功路徑零開銷：URL 連得上就不掃描；sticky 原則不變。
- 存量已配對裝置免重配對：下一次成功認證連線自動補 pin 指紋。

**Non-Goals:**

- 不做挑戰-回應身份證明、不引入 TLS（獨立後續）。
- 不改引導式 init 的選單流程（它已用收集式掃描；本變更只讓它多帶指紋）。
- 不處理「server 換了身份 id」的自動遷移（視為異常：警告、需人工重配對）。

## Decisions

### 決策一：server 持久身份 id

首次啟動生成 UUID 存 `<agent home>/server-id`（0600 不必要——非秘密，但沿一般檔案即可），之後每次啟動讀同一值；讀寫失敗降級為「本次執行的暫時 id」並記警告（永不崩潰）。指紋值=該 id 字串本身（明文識別子，無需雜湊——不含秘密）。

### 決策二：兩處曝露——mDNS TXT 與 Welcome

- mDNS TXT 加 `fp=<server-id>`（既有 `version` 屬性旁）。
- `Welcome` 加 `server_fingerprint: Option<String>`（serde default、None 不序列化，additive）。TXT 供「連不上時的掃描比對」；Welcome 供「已認證連線上的 pin 與校正」——兩者必一致（同一來源值）。

### 決策三：pin 的時機——配對時與已認證連線時（TOFU-on-connect）

- 配對成功（Welcome 帶 token）：連同 token 一起把 server_fingerprint 寫進 current profile（CLI 的 pair、fleetyd 的 persist_token 兩處）。
- 存量裝置：任何一次成功認證的連線收到 Welcome.server_fingerprint 時，若 current profile 尚無指紋 → 補 pin（信任已認證連線）；若已有且不同 → 警告「server identity changed」且**不覆寫**（異常訊號，指引重新配對）。

### 決策四：黏著自癒的觸發與比對

觸發條件（全部成立才掃）：以 current profile 的 URL 連線失敗、profile 有 pinned 指紋、`FLEETY_MDNS_DISABLED` 未設。動作：跑一次收集式掃描（沿 3 秒窗口），在結果中找 TXT 指紋==pinned 的項目：
- 找到且 URL 與存的不同 → 更新 profile.url（持久化）、印「server '<profile>' moved to <url> (same identity fingerprint); reconnecting」、以原 token 重連新 URL。
- 找到但 URL 相同（server 沒動，只是暫時斷線）→ 不改檔，維持原錯誤/重試。
- 沒找到相符者 → 維持原錯誤/退避；指紋不符的發現者完全忽略。
CLI：包在一次性連線的失敗路徑（open 失敗後 heal 一次，成功則重連一次，再失敗即報原錯誤——不迴圈）。fleetyd：重連退避迴圈中，每次連線失敗後、退避睡眠前執行同一 heal（找到即立即以新 URL 重試）。

### 決策五：heal 的守門與純函式切分

「哪個發現者可以接受」抽為純函式：輸入 pinned 指紋與發現清單，輸出可採用的新 URL（Option）——指紋完全相等才回 Some，任何 None/不等皆 exclude；單元測試涵蓋相符、不符、缺席、多台混雜（只挑相符那台）。profile 寫回沿既有 connection::save。

## Implementation Contract

**行為（操作者視角）：**

- server 換網路後：已配對裝置下一次連線（CLI 指令或 fleetyd 重連）短暫掃描後自動接上，印一行「server moved to <新url>」，token 原樣沿用，之後恢復 sticky。
- LAN 上同時有第二台 fleety server：其指紋不同，自癒永不採用它；token 不會送給它。
- 舊 server（無指紋廣播）：新裝置行為與現狀一致（連不上就是連不上，不誤採任何廣播者）。
- server 身份檔被刪/重建（指紋變了）：已 pin 裝置連上時收到不同指紋 → 警告不覆寫；自癒掃描比對不上 → 不採用。人工重跑 init/pair 後恢復。

**介面與資料形狀：**

- server：`server-id` 檔（agent home 下，一行 UUID）；mDNS TXT `fp`；`Welcome.server_fingerprint: Option<String>`。
- `Discovered { url, fingerprint: Option<String> }`（既有形狀，開始被真值填充）；`DiscoveredServer`（收集式）加 `fingerprint: Option<String>` 欄位。
- 純函式：`heal_candidate(pinned: &str, found: &[…]) -> Option<String>`（命名 apply 時對齊）；profile pin 寫入 helper（pair 與 TOFU 共用）。
- fleety-tools connection：新增「以指紋更新 current profile url」與「pin 指紋」的儲存函式，簽名 apply 時定。

**失敗模式：**

- server-id 檔不可讀寫 → 暫時 id＋警告（該次執行內一致；重啟後變動會使已 pin 裝置的自癒失效直到修復——警告文字指出檔案路徑）。
- 掃描不可用（daemon 建立失敗）→ 視同無發現。
- 自癒重連仍失敗 → 報原始連線錯誤（URL 已更新為新值，下次重試自然用新址）。

**驗收準則：**

- cargo test：heal 候選純函式（相符/不符/缺席/多台混雜）；TOFU pin 規則（無→補 pin、同→no-op、異→警告不覆寫）純函式化並測試；TXT 解析出 fingerprint 的收集邏輯測試；Welcome 欄位 additive round-trip（protocol 測試）。
- 既有 resolver 測試不回歸（sticky 語義不變）。
- cargo clippy -D warnings、fmt 乾淨；全 workspace 測試綠。
- 端到端（發版後人工）：Mac mini 換 Wi-Fi/IP 後，Windows CLI 直接下指令自動接上。

**範圍邊界：**

- 範圍內：crates/fleety-server/src/{mdns.rs,main.rs,conn.rs}、crates/fleety-protocol/src/lib.rs、crates/fleety-tools/src/connection.rs、crates/fleety-cli/src/main.rs、crates/fleety-daemon/src/main.rs、docs/env.md。
- 範圍外：TLS、挑戰式驗證、resolver 優先序、引導式 init 的 UX。

## Risks / Trade-offs

- [明文指紋可被複製假冒] → 不劣化現狀（明文 LAN 竊聽者本就見 token）；文件明載信任邊界；TLS/挑戰驗證為既有後續。
- [heal 掃描把一次性 CLI 指令拖慢 3 秒] → 只發生在「本來就要失敗」的路徑上，成功路徑零開銷；可接受。
- [server-id 檔遺失造成指紋輪替] → 警告與文件指引（重新 pair）；不自動遷移是刻意保守。

## Migration Plan

單版出貨。存量裝置無需動作：下一次成功連線自動補 pin，之後即享自癒。回滾 revert 即可（pinned 指紋欄位閒置無害）。

## Open Questions

- 無阻斷項。
