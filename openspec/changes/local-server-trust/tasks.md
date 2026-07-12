## 1. Server loopback 信任

- [x] 1.1 依 design「決策一:loopback 信任在 authenticate,peer 位址由 ConnectInfo 帶入」與 spec 的 Loopback connections are trusted on the same host 要求:crates/fleety-server/src/conn.rs 的 authenticate 與 run_connection 增 peer_is_loopback 參數,loopback+require_auth+trust 時免 token 放行;peer_is_loopback 判定與 trust_loopback_enabled(FLEETY_TRUST_LOOPBACK,預設真、0 關)抽純函式。先寫測試(tdd):peer 分類(v4/v6 loopback vs LAN)、authenticate 四分支(loopback 放行、trust 關拒、遠端拒、有效 token 照舊)、env 解析。驗證:cargo test -p fleety-server 全綠、既有 auth 測試不回歸。
- [x] 1.2 依 design「決策一」把 peer 位址由 axum ConnectInfo 帶入:serve 改 into_make_service_with_connect_info::<SocketAddr>();http.rs 的 ws_handler 與 SSE/send handler 取 ConnectInfo、算 peer_is_loopback、傳進 run_connection 兩個呼叫點。驗證:cargo build -p fleety-server 通過、cargo test -p fleety-server 全綠。

## 2. Welcome 旗標(protocol + server)

- [x] 2.1 依 design「決策二:Welcome 帶回 loopback 信任旗標」與 spec 要求:fleety-protocol 的 Welcome 增 loopback_trusted: bool(serde default、additive);server 在以 loopback 信任放行時設真。先寫測試:Welcome additive round-trip、舊形反序列化為 false。驗證:cargo test -p fleety-protocol -p fleety-server 全綠。

## 3. install-server.sh 裝 CLI

- [x] 3.1 依 design「決策三:install-server.sh 一併裝 CLI」與 spec 的 The server installer also installs the CLI 要求:install-server.sh 裝完 server/sidecar 後,以同 target 邏輯抓 fleety 資產裝到同 dir(best-effort,失敗印手動安裝提示);尾段導引改指向 fleety init。驗證:sh -n 通過、內容審閱涵蓋兩個 scenario。

## 4. CLI 本機預設 profile

- [x] 4.1 依 design「決策四:CLI 把本機 server 當一級預設可切換 profile」與 spec 的 The local server is a first-class default switchable profile / Guided init probes the local server before scanning 要求:引導式 init 先以短逾時探測本機 server(連 ws://127.0.0.1:<port>,port 取 FLEETY_ADDR 或預設),有 Welcome 就當 DiscoveredServer(名稱 local、置選單頂端且預設);選 local 存 local profile、設 current、跳過配對碼(loopback 信任);無本機 server 則安靜略過照舊。本機 URL 推導(port 取用)抽純函式。先寫測試:port 推導、local 條目置頂/預設選擇、選 local 不提示配對的分支判斷。驗證:cargo test -p fleety-cli 全綠。

## 5. 文件

- [x] 5.1 [P] docs/env.md:FLEETY_TRUST_LOOPBACK(預設信任、0 關、代理注意)、install-server.sh 一併裝 CLI、fleety init 的本機預設流程、威脅模型說明。驗證:內容審閱與 spec 用語一致。

## 6. 整體驗證

- [x] 6.1 全 workspace 驗證:cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過。驗證:指令輸出乾淨。
