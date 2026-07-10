## Context

現況(讀程式碼驗證):`fleety-tools` config.rs registry 的 `FLEETY_ADDR` 預設 `"127.0.0.1:8787"`;`fleety-server` main.rs 讀 `std::env::var("FLEETY_ADDR").unwrap_or_else(|_| "127.0.0.1:8787")`(seed_env_from_config 先跑,config.toml 值進 env,皆未設才落 fallback);bind 後若 addr 以 `127.0.0.1`/`localhost` 開頭會 log「bound to loopback — other devices cannot reach this server」提示。`mdns.rs` 的 `local_ips(bind_addr)`:先看 `FLEETY_MDNS_HOST_IP`(顯式 → 用),否則 parse bind_addr,若 IP `is_unspecified()`(0.0.0.0)或 `is_loopback()` → **回空 Vec**;`spawn_advertise` 對空 IP 直接跳過 mDNS 廣播。故現況綁 0.0.0.0 時 mDNS 不廣播(除非設 HOST_IP)。安全面:`auth-default-on` 已上,`FLEETY_REQUIRE_AUTH` 預設開、首啟印配對碼。

## Goals / Non-Goals

**Goals:** 裸機 server 開箱即跨裝置可達(預設 `0.0.0.0`)且自動發現仍可用(綁 0.0.0.0 時自動偵測對外 IP 廣播 mDNS)。

**Non-Goals:**（見 proposal)mDNS 多台選單 UI、wss/TLS 硬要求、清理 Docker 冗餘 0.0.0.0。

## Decisions

### FLEETY_ADDR 裸機預設翻為 0.0.0.0

registry(config.rs)`FLEETY_ADDR` 預設 `"127.0.0.1:8787"`→`"0.0.0.0:8787"`,描述註明「預設對外監聽(auth 預設開、首啟印配對碼);設 127.0.0.1 只限本機」。main.rs 讀取的 fallback 同步改 `"0.0.0.0:8787"`。因 env>config>default 一致落在 0.0.0.0,顯式設 127.0.0.1/其他值者不受影響,只有「完全沒設」的預設翻為對外。

### 綁 0.0.0.0 時自動偵測對外 IP(mDNS 才能廣播)

`mdns.rs` 的 `local_ips`:當 bind IP 為 unspecified(0.0.0.0)且未設 `FLEETY_MDNS_HOST_IP` 時,呼叫新 helper `detect_route_ip()` 自動取一個對外 IP——開一個 `UdpSocket::bind("0.0.0.0:0")` 後 `connect("8.8.8.8:80")`(UDP connect 不實際送封包,只讓 OS 依路由選出對外介面的 local IP),讀 `local_addr().ip()`;結果若 loopback/unspecified 則捨棄(回 None)。偵測到 → 廣播該 IP;偵測不到 → 回空(維持現況跳過廣播)。`FLEETY_MDNS_HOST_IP` 顯式設仍優先(多網卡覆寫)。loopback bind 仍回空(不廣播,語意不變)。IPv6/無網路等偵測失敗只是回空,不 panic、不阻擋啟動。

### loopback 提示保留(只在顯式 loopback 時觸發)

main.rs 的 `addr.starts_with("127.0.0.1") || addr.starts_with("localhost")` 提示保留。改預設 0.0.0.0 後此提示只在使用者**顯式**綁 loopback 時出現,是正確信號(提醒「你綁本機,別台連不到」)。不改此邏輯。

## Implementation Contract

**行為:**
- 未設 `FLEETY_ADDR` 起 server → 綁 `0.0.0.0:8787`(對外);`fleety config get FLEETY_ADDR` 顯示預設 `0.0.0.0:8787`。
- 綁 0.0.0.0 且未設 `FLEETY_MDNS_HOST_IP` 且有對外網路 → mDNS 廣播一個對外(非 loopback/非 0.0.0.0)IP,別台 `fleety`/`fleetyd` 可自動發現。
- 綁 0.0.0.0 且**已設** `FLEETY_MDNS_HOST_IP` → 廣播該指定 IP(覆寫偵測)。
- 顯式綁 `127.0.0.1:8787` → 不廣播 mDNS(local_ips 回空)+ 出現 loopback 提示。
- 偵測不到對外 IP(無網路)→ 不廣播、不 panic、server 照常起。
- 顯式設 `FLEETY_ADDR` 為任意值者行為不變(只有未設的預設翻)。

**介面 / 資料形狀:**
- registry `FLEETY_ADDR` default `"0.0.0.0:8787"`。
- main.rs addr fallback `"0.0.0.0:8787"`。
- `mdns.rs`:`local_ips(bind_addr)` 於 unspecified 分支呼叫新 `fn detect_route_ip() -> Option<std::net::IpAddr>`(UDP-connect 偵測)。

**失敗模式:**
- UDP socket 建立/connect 失敗、無路由、IPv6-only 未涵蓋 → `detect_route_ip` 回 None → local_ips 回空 → 跳過廣播(不 panic)。
- 偵測到 loopback/unspecified → 捨棄回 None。

**驗收條件:**
- fleety-tools 單元:`FLEETY_ADDR` 預設 resolve 為 `"0.0.0.0:8787"`。
- fleety-server 單元:`local_ips("0.0.0.0:8787")` 未設 HOST_IP 時回非空且元素非 loopback/非 unspecified(有網路時);設 `FLEETY_MDNS_HOST_IP` 時回該值;`local_ips("127.0.0.1:8787")` 回空;`detect_route_ip()` 回 None 或一個非 loopback IP(不 panic)。
- `cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-server` 無新違規。
- 手動:未設 FLEETY_ADDR 起 server,log 顯示 bound 0.0.0.0 + mDNS advertising 一個 LAN IP(非 loopback)。

**範圍邊界:**
- In scope:FLEETY_ADDR 預設翻 0.0.0.0(registry + main.rs)+ mDNS 綁 0.0.0.0 自動偵測對外 IP + spec/docs 更新。
- Out of scope:mDNS 多台選單、wss/TLS、Docker 清理、介面全列舉。

## Risks / Trade-offs

- [預設對外的安全] 監聽全介面 → 但 auth 預設開(需 token)+ 首啟配對碼 → 「能連但要配對」;顯式關 auth 者仍可設 127.0.0.1 或 REQUIRE_AUTH=0。此三者已由 auth-default-on 對齊,風險可接受。
- [UDP-connect 偵測不準/多網卡] 偵測選的是「預設路由的對外介面」IP,單網卡正確;多網卡/VPN 可能選錯 → `FLEETY_MDNS_HOST_IP` 覆寫保留為逃生門。
- [IPv6-only 環境] connect IPv4 8.8.8.8 可能失敗 → 回 None → 跳過廣播(退化為現況,不 panic);IPv6 廣播可後續加。
- [8.8.8.8 依賴] 只用來讓 OS 選路由,不送封包、不需可達;離線也只是回 None。可改任意公網 IP(如文件保留數字),不做 DNS。
