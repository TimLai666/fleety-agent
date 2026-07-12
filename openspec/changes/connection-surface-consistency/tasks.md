## 1. 共用探測 helper

- [x] 1.1 依 design「決策一:探測 helper 提升為 pub(crate) 共用」把 crates/fleety-cli/src/main.rs 的 local_server_url 與 probe_local_server 改為 pub(crate),行為不變,讓 config_panel 重用(單一實作,面板與 init 對「本機 server」一致)。驗證:cargo build -p fleety-cli 通過、既有 init 相關測試不回歸。

## 2. 面板 Connection 區列本機

- [x] 2.1 依 design「決策二:面板開啟時注入 local 條目」與 spec 的 The panel Connection region offers the local server 要求:config_panel::run() 載入 conns 後以短逾時探測本機,若有回應且無 profile 指向本機 url 就在記憶體 conns 插入 local profile(不落地,除非使用者存);Connection 區說明行補本機免配對提示;無本機或已有對應 profile 則不插入。是否已有本機 profile 的判定抽純函式。先寫測試(tdd):has_local_profile 純函式(有/無對應 url)。驗證:cargo test -p fleety-cli 全綠、既有面板測試不回歸。

## 3. connect_hello 可讀認證錯誤

- [x] 3.1 依 design「決策三:connect_hello 可讀認證錯誤」與 spec 的 One-shot commands surface authentication rejections readably 要求:connect_hello 非 Welcome 分支細分——unauthenticated Error 回 not-paired 可讀訊息(含 fleety pair / fleety pair-code 指引)、其他 Error 回 server 訊息、其他 frame 回一般可讀訊息;沿用 is_auth_rejection。先寫測試:錯誤映射(unauthenticated→not-paired 文案、其他 Error→server 訊息、其他→一般)以訊息組裝純函式覆蓋。驗證:cargo test -p fleety-cli 全綠。

## 4. 整體驗證

- [x] 4.1 全 workspace 驗證:cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過。驗證:指令輸出乾淨。
