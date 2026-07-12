## Why

在 server 本機執行 `fleety tui`(或任何連線指令、無 profile 時)會失敗:mDNS 探測到的是**本機自己的對外 LAN IP**(例如 `ws://192.168.1.109:8787`),而 CLI 的解析順序是「profile → mDNS → localhost 預設」,mDNS 先命中就永遠走不到最後的 loopback 預設。

問題在於同機到 LAN IP 的連線,server 端看到的 peer **不是 loopback**,所以不觸發「同機自動信任」(`peer_is_loopback && trust_loopback_enabled()`);於是無 token + auth 預設開 → 連線被拒 → TUI 開不起來。使用者被迫在 server 本機也去 `init`/配對,這不合理:同機的 CLI 本該經 `127.0.0.1` 自動受信任、免配對。

Guided `init` 已經有「本機優先探測」,但一般連線解析路徑漏了這一步。

## What Changes

- CLI 的連線解析,在 discovery 這一層(no override/env/sticky profile 時)**先探測本機 loopback server**(對 `127.0.0.1:<port>` 做一次短逾時 TCP 連線),有回應就用 `ws://127.0.0.1:<port>`(同機自動信任、免 token),**mDNS 只在本機沒有 server 時才用**。
- 解析出的是本機 loopback 時,提示訊息改為「using this host's local server … same-host trusted」,而非誤導的「discovered agent on the LAN」。
- 僅改 fleety CLI 的解析;daemon 的解析行為不變(它靠 enrolled profile,sticky 先勝)。

## Non-Goals (optional)

- 不改 fleety-tools 共用 `connection::resolve` 的簽章或 daemon 行為(loopback 優先只做在 CLI 注入的 discovery 閉包裡)。
- 不改 server 端 loopback 信任機制、mDNS 廣播或 fingerprint guard。
- 不做 loopback 的完整 WS/Welcome 握手探測(用輕量 TCP 連線判存活即可,真正握手由後續實際連線完成)。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `service-discovery`: resolver 的 discovery fallback 順序,由「profile → mDNS → localhost 預設」改為在 CLI 端「profile → 本機 loopback 探測 → mDNS → localhost 預設」;明訂同機 CLI 優先連自己的 loopback server 以取得同機信任。

## Impact

- Affected specs: `service-discovery`(modified)
- Affected code:
  - Modified:
    - crates/fleety-cli/src/main.rs — 新增 loopback_server_up / prefer_loopback_discovery,接進 resolve_target 的 discovery 閉包;修正 loopback 命中時的提示
  - New: (none)
  - Removed: (none)
