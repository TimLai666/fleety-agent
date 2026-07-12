## Context

`fleety init` 引導選單(local-server-trust 出貨)已有 `local_server_url()`(從 FLEETY_ADDR 推本機 ws URL)與 `probe_local_server(url, timeout)`(連 loopback、送 Hello、收 Welcome → Option<DiscoveredServer>),但都是 main.rs 私有。設定面板 `config_panel::run()`(async)載入 `conns`(connections.toml profiles),Connection 區列 `p.conns.profiles`、`u` 設 current、`s` 存檔;它不探測、不掃描。`connect_hello`(main.rs)握手後只 match `Welcome`,其餘落 `other => Err("expected welcome, got {other:?}")`——Debug dump。`is_auth_rejection(kind)`(enrollment-reconnect-ux 出貨,TUI 用)判定 `unauthenticated`。device-enrollment spec 已有「Pairing failures surface readable errors」要求 `fleety pair` 不印 Debug;本變更把同精神延伸到 connect_hello。

## Goals / Non-Goals

**Goals:**
- 設定面板能像 init 一樣看到並選到本機 server(免配對)。
- 一次性指令的認證拒絕給可讀訊息,不印 Debug。

**Non-Goals:**
- 不在面板加 mDNS LAN 掃描(本變更只補「本機」;LAN 掃描選單是 init 的職責,面板保持輕量)。
- 不改 wire/protocol、不改 loopback 信任本身。
- 不改 connect_hello 對成功握手與非認證錯誤的行為。

## Decisions

### 決策一:探測 helper 提升為 pub(crate) 共用

`local_server_url()` 與 `probe_local_server()` 由 main.rs 私有改 `pub(crate)`,供 config_panel 重用——單一實作,面板與 init 對「本機 server 是什麼、如何探測」一致(正是 AGENTS.md 平行面規則)。否決在 config_panel 複製一份:兩份必漂移。

### 決策二:面板開啟時注入 local 條目

`config_panel::run()` 載入 conns 後,以短逾時(~1s)`probe_local_server(local_server_url())`;若有回應且 `conns.profiles` 無任何 profile 的 url 等於本機 url,就插入一個名為 `local` 的 profile(url=本機、無 token)到記憶體 conns。它因而出現在 Connection 區(既有渲染 name+url),`u` 設 current、`s` 存檔即持久化。Connection 區的說明行補一句提示本機免配對。無本機 server 或已有對應 profile → 不插入,行為不變。

- 記憶體注入不落地,除非使用者 `s`——與「選了才存」一致,不在唯讀開面板時偷寫檔。

### 決策三:connect_hello 可讀認證錯誤

`connect_hello` 的非 Welcome 分支細分:`Some(ServerMsg::Error { error })` 且 `is_auth_rejection(&error.kind)` → 回可讀「尚未與此 server 配對,請 `fleety pair <code>`(可用 `fleety pair-code` 在 server 主機生碼)」;其他 `Error` → 回 server 的 error.message(可讀,非 Debug);其他 frame → 一般可讀「unexpected reply from server」。`is_auth_rejection` 純函式(已存在)共用。此改動涵蓋所有走 connect_hello 的一次性指令。

## Implementation Contract

**行為(操作者視角):**
- server 主機從沒 init 過,開 `fleety config`:Connection 區頂端出現 `local  ws://127.0.0.1:<port>`,按 `u`+`s` 即切到本機並存檔,免配對,之後面板 Server 區可編輯本機設定。
- 無本機 server 的主機開面板:Connection 區與現況完全一致(只列既有 profile)。
- 未配對裝置跑 `fleety pair-code`(或 status/audit/…):看到「尚未與此 server 配對,請 fleety pair …」的可讀訊息,非 `Some(Error { kind: "unauthenticated", … })`。
- 已配對/loopback 連線:一切照舊。

**介面與資料形狀:**
- `pub(crate) fn local_server_url() -> String`;`pub(crate) async fn probe_local_server(&str, Duration) -> Option<DiscoveredServer>`(既有,改可見性)。
- config_panel 在 run() 注入 local profile 的邏輯;是否已有本機 profile 的判定抽純函式(`has_local_profile(&Connections, &str) -> bool` 之類,命名 apply 時定)。
- connect_hello 的錯誤映射;沿用 `is_auth_rejection`。

**失敗模式:**
- 本機探測逾時/失敗 → 面板不加 local 條目,不報錯。
- connect_hello 連不上 → 既有 open() 錯誤(不變)。

**驗收準則:**
- cargo test:has_local_profile 純函式(有/無對應 url);connect_hello 錯誤映射的可讀性以純函式或訊息組裝測試(unauthenticated → not-paired 文案、其他 Error → server 訊息、其他 → 一般訊息);既有面板與 CLI 測試不回歸。
- 面板 local 注入的可測部分(注入判定)有測試;互動渲染沿專案既有面板測試姿態。
- 全 workspace test/clippy/fmt 乾淨。
- 端到端(發版後人工):server 主機開 `fleety config` 選 local;未配對跑 pair-code 見可讀訊息。

**範圍邊界:**
- 範圍內:crates/fleety-cli/src/main.rs、crates/fleety-cli/src/config_panel.rs。
- 範圍外:mDNS 面板掃描、wire/protocol、loopback 信任、daemon。

## Risks / Trade-offs

- [面板注入 local 條目但使用者未存 → 下次開又要重探] → 探測便宜(~1s、僅開面板時);存了就變常駐 profile。可接受。
- [connect_hello 文案改動影響多個指令] → 都是把難懂 Debug 換可讀訊息,只會更好;非認證路徑不動。

## Migration Plan

單版出貨,無資料遷移。回滾 revert。

## Open Questions

- 無阻斷項。
