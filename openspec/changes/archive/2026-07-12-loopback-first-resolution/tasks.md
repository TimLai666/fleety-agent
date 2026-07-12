## 1. CLI loopback-first 解析(crates/fleety-cli/src/main.rs)

- [x] 1.1 依 spec「The CLI prefers a co-located loopback server over mDNS」:新增 loopback_server_up(timeout)->Option<String>(對 127.0.0.1:<port> 短逾時 TCP 連線判存活,回 loopback URL)與純函式 prefer_loopback_discovery(loopback, mdns)->Option<Discovered>(loopback 先、mDNS 後);接進 resolve_target 的 discovery 閉包;resolve 出 loopback 時提示改為「using this host's local server … same-host trusted」而非「discovered agent on the LAN」。單元測試:loopback Some 勝過 mDNS、loopback None 落到 mDNS、兩者 None 回 None。驗證:cargo test -p fleety-cli loopback_wins_over_mdns 全綠。

## 2. 驗證

- [x] 2.1 全 CLI 驗證:cargo test -p fleety-cli、cargo clippy -p fleety-cli --all-targets -- -D warnings、cargo fmt --all -- --check 乾淨。驗證:指令輸出乾淨。
