## 1. server 身份與曝露

- [x] 1.1 依 design「決策一：server 持久身份 id」與 spec 的 The server advertises a persistent identity fingerprint 要求：server 首啟生成 UUID 存 agent home 的 server-id 檔、重啟重讀同值、讀寫失敗降級為本次執行暫時 id 並警告（不崩潰）。先寫測試（tdd）：載入-或-生成的持久性（同路徑兩次呼叫同值）、壞路徑降級。驗證：cargo test -p fleety-server 全綠。
- [x] 1.2 依 design「決策二：兩處曝露——mDNS TXT 與 Welcome」：crates/fleety-server/src/mdns.rs 的 TXT 加 fp 屬性；fleety-protocol 的 Welcome 加 server_fingerprint（serde default、None 不序列化、additive）；server conn 的 Welcome 組裝帶入同一值。先寫測試：Welcome round-trip 含新欄位、舊形 JSON 反序列化為 None。驗證：cargo test -p fleety-protocol 與 -p fleety-server 全綠。

## 2. 指紋 pin（fleety-tools／CLI／daemon）

- [x] 2.1 依 design「決策三：pin 的時機——配對時與已認證連線時（TOFU-on-connect）」與 spec 的 Server fingerprints are pinned at pairing and on authenticated connections 要求：fleety-tools connection 新增 pin 儲存 helper 與 TOFU 規則純函式（無→補 pin、同→no-op、異→警告不覆寫）；CLI 的 pair 與 fleetyd 的 persist_token 在配對時連指紋一起寫；CLI 與 fleetyd 在成功認證連線收到 Welcome 指紋時套 TOFU 規則。先寫測試：TOFU 三分支純函式、pair 寫入含指紋。驗證：cargo test -p fleety-tools -p fleety-cli -p fleety-daemon 全綠。

## 3. 黏著自癒

- [x] 3.1 依 design「決策四：黏著自癒的觸發與比對」「決策五：heal 的守門與純函式切分」與 spec 的 Sticky connections heal by fingerprint when the address moves 要求：收集式掃描解析 TXT fp（DiscoveredServer 加 fingerprint 欄位）；heal 候選純函式（pinned 對發現清單 → 僅完全相符者回新 URL）；CLI 一次性連線失敗路徑 heal 一次（更新 profile url、印 server moved、重連一次）；fleetyd 重連迴圈每次失敗後退避前 heal。先寫測試：heal 候選（相符/不符/缺席/多台混雜只挑相符）、TXT 指紋進入收集項。驗證：cargo test -p fleety-cli -p fleety-daemon 全綠，resolver 既有 sticky 測試不回歸。

## 4. 文件

- [x] 4.1 [P] docs/env.md：mDNS 段補指紋屬性與自癒行為（觸發條件、只認相符指紋、token 不外流原則）、信任邊界說明（明文 LAN；TLS/挑戰驗證為後續）、server-id 檔位置與遺失時的重配對指引。驗證：內容審閱與 spec 用語一致。

## 5. 整體驗證

- [x] 5.1 全 workspace 驗證：cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過。驗證：指令輸出乾淨。
