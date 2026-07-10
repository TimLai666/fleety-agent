## 1. 預設翻 0.0.0.0(FLEETY_ADDR 裸機預設翻為 0.0.0.0)

- [x] 1.1 [P] 在 crates/fleety-tools/src/config.rs 把 registry 的 `FLEETY_ADDR` 預設 `"127.0.0.1:8787"`→`"0.0.0.0:8787"`、描述改為「預設對外監聽(auth 預設開、首啟印配對碼);設 127.0.0.1:8787 只限本機」(design「FLEETY_ADDR 裸機預設翻為 0.0.0.0」),交付「Server bootstrap configuration」的預設面。單元(serial,先 remove FLEETY_ADDR env):`resolve("FLEETY_ADDR", empty)` 值為 `"0.0.0.0:8787"`、source default。
- [x] 1.2 [P] 在 crates/fleety-server/src/main.rs 把讀取 `FLEETY_ADDR` 的 `unwrap_or_else` fallback `"127.0.0.1:8787"`→`"0.0.0.0:8787"`(與 registry 一致)(design「FLEETY_ADDR 裸機預設翻為 0.0.0.0」),交付「Server bootstrap configuration」runtime 端;loopback 提示邏輯(`addr.starts_with("127.0.0.1")||"localhost"`)不動(design「loopback 提示保留(只在顯式 loopback 時觸發)」)。以既有 server 啟動/smoke 測試不因預設翻而壞;`invalid_bind_exits_after_startup_setup`(顯式設 FLEETY_ADDR)不受影響。

## 2. mDNS 綁 0.0.0.0 自動偵測(綁 0.0.0.0 時自動偵測對外 IP(mDNS 才能廣播))

- [x] 2.1 [P] 在 crates/fleety-server/src/mdns.rs 新增 `fn detect_route_ip() -> Option<std::net::IpAddr>`(`UdpSocket::bind("0.0.0.0:0")` 後 `connect("8.8.8.8:80")`,不送封包,讀 `local_addr().ip()`,loopback/unspecified 捨棄回 None,任何 IO 失敗回 None 不 panic),並在 `local_ips` 的 unspecified(0.0.0.0)分支「未設 FLEETY_MDNS_HOST_IP 時」呼叫它;HOST_IP 顯式設仍優先、loopback bind 仍回空(design「綁 0.0.0.0 時自動偵測對外 IP(mDNS 才能廣播)」),交付「mDNS service discovery」的 0.0.0.0 自動偵測條款。單元:`local_ips("0.0.0.0:8787")` 未設 HOST_IP 時回空或全非 loopback/非 unspecified(不 panic)、設 `FLEETY_MDNS_HOST_IP` 時回該值、`local_ips("127.0.0.1:8787")` 回空、`detect_route_ip()` 回 None 或一個非 loopback IP。

## 3. 文件與驗證

- [x] 3.1 更新 docs/env.md(FLEETY_ADDR 預設值改 0.0.0.0:8787、`FLEETY_MDNS_HOST_IP` 條件由「required when 0.0.0.0」改為「override; 0.0.0.0 時自動偵測對外 IP」)與 docs/roadmap.md(待決策略移除 FLEETY_ADDR 這項、註記已出貨);全回歸:`cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-server` 無新違規;手動:未設 FLEETY_ADDR 起 server,log 顯示 bound 0.0.0.0 + mDNS advertising 一個 LAN IP(非 loopback)。驗證:測試輸出 + 啟動 log 手動檢查。
