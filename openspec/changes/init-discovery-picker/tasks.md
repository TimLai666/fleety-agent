## 1. 發現層（fleety-cli）

- [x] 1.1 依 design「決策二：多台收集的發現函式」與 spec 的 Interactive discovery lists every advertised server 要求，在 crates/fleety-cli/src/main.rs 新增 DiscoveredServer 與 discover_all_via_mdns（固定 3 秒窗口、URL 去重、發現順序、FLEETY_MDNS_DISABLED 或 daemon 失敗回空清單），顯示名剝 fleety- 前綴、缺名退 URL 的推導抽為純函式。先寫測試（tdd）：名稱推導（含前綴剝除與缺名退 URL 兩型）、URL 去重的收集邏輯（以事件序列切分的純函式測）。驗證：cargo test -p fleety-cli 新增測試全綠，既有 discover_via_mdns 未改動。

## 2. 引導流程（fleety-cli）

- [x] 2.1 依 design「決策一：入口是 init 無參數加 TTY」「決策三：行輸入選單」「決策四：profile 建立沿 upsert 保留 token」「決策五：配對引導可跳過」與 spec 的 Guided first-run init discovers, picks, and pairs 要求，改造 init dispatch：無 URL＋TTY＋mDNS 未禁用 → 掃描、編號清單（含 saved 標記）、行輸入選擇（空/EOF 取消、非法重提示）、upsert profile 設 current（--name 覆蓋預設顯示名）、配對碼提示（空跳過並印 fleety pair 提示；有碼走既有 pair，失敗保留 profile）。選擇解析與清單渲染抽為純函式。先寫測試：選擇解析（合法/超界/非數字/空）、清單渲染含 saved 標記、profile 名預設推導。驗證：cargo test -p fleety-cli 全綠。
- [x] 2.2 fallback 邊界維持不變：掃描無結果印無發現訊息加既有 usage、非 TTY 或 FLEETY_MDNS_DISABLED 直接既有 usage、fleety init <ws-url> 行為與訊息完全不變。驗證：cargo test -p fleety-cli 既有 init 相關測試全綠，usage 文案人工比對。

## 3. 文件

- [x] 3.1 [P] docs/env.md 與 README.md 的上手段落更新：第一次流程改為 fleety init（無參數）的掃描選擇配對引導，顯式 URL 與 pair 用法保留說明。驗證：內容審閱與 spec 用語一致。

## 4. 整體驗證

- [x] 4.1 全 workspace 驗證：cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過。驗證：指令輸出乾淨。
