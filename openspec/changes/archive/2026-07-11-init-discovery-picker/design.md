## Context

現況三個事實：resolver 的隱式 mDNS fallback（無覆蓋、無 env、無 profile 時）取**第一台** resolved 的 `_fleety._tcp.local.` 就用，過程對使用者不可見；`fleety init` 必須帶 `ws://` URL，無參數直接回 usage 錯誤；server 端廣播已帶可讀 instance 名（`fleety-<hostname>`，`FLEETY_MDNS_HOST` 可覆蓋）。CLI 的發現函式（discover_via_mdns，crates/fleety-cli/src/main.rs）以 mdns-sd 瀏覽、收到第一個 ServiceResolved 即回傳 URL。profile 建立走 upsert_profile_and_use（保留既有 token）、配對走既有 async pair(code)。

## Goals / Non-Goals

**Goals:**

- 第一次上手一條龍：`fleety init`（無參數、TTY）→ 掃描清單 → 選擇 → 建 profile 設 current → 引導輸入配對碼 → 完成。
- 多台 server 同網段時使用者能看見並選擇；單台時同一流程（清單一項）。
- 顯式 `fleety init <ws-url>`、`fleety pair`、resolver 隱式 fallback 完全不變。

**Non-Goals:**

- 不做 ratatui 全螢幕選單（一次性選擇用行輸入即可，與 pair 的簡樸互動一致）。
- 不改 server 端廣播內容。
- 不在此輪把指紋 pin 帶進選單流程（隱式 fallback 的 fingerprint guard 機制不動）。
- 不處理跨網段發現（mDNS 天生同網段；跨網段維持手動 init URL）。

## Decisions

### 決策一：入口是 init 無參數加 TTY

引導流程只接管「`fleety init` 沒給 URL 且 stdout 是 TTY」——這個入口現在必然以 usage 錯誤收場，接管零相容性風險。非 TTY、掃描無結果、或 `FLEETY_MDNS_DISABLED` 時維持現行 usage 提示。否決「fleety pair 也彈選單」：pair 已有明確語義（對 resolver 選定的 server 配對），一個入口學一件事。

### 決策二：多台收集的發現函式

新增 discover_all_via_mdns（固定收集窗口，預設 3 秒）：瀏覽整個窗口、收集**每個** ServiceResolved、以 URL 去重、每台帶顯示名（instance 名去掉 `fleety-` 前綴；缺名時用 URL 代替）。回傳 Vec，順序=發現順序。現行單台 discover_via_mdns 保留不動（resolver 隱式 fallback 用它，2 秒早退語義維持）。

### 決策三：行輸入選單

清單以編號列出（名稱＋URL＋既有 profile 標記「(saved)」），stdin 讀一行編號；空行或 EOF 視為取消（印 usage 提示離開）、非法編號重新提示。一次性選擇不值得 ratatui 的進出成本；行輸入在 SSH/管線環境也最穩。

### 決策四：profile 建立沿 upsert 保留 token

選定後 profile 名預設=顯示名（`--name` 覆蓋），走既有 upsert_profile_and_use：同名 profile 只更新 URL、**保留既有 token**（重跑 init 不會弄丟已配對狀態）並設 current。

### 決策五：配對引導可跳過

建立 profile 後提示輸入配對碼（提示語指出碼的來源：server 首次啟動的 console、或已配對裝置請 agent 用 pair_create 生碼）；輸入空行=跳過並印出「之後可 fleety pair <code>」；有輸入即呼叫既有 pair 流程，成功印「paired; '<profile>' is now your current server」。配對失敗不回滾 profile（profile 本身有效，重試 pair 即可），錯誤沿既有可讀訊息。

## Implementation Contract

**行為（操作者視角）：**

- 同網段有 server 時：`fleety init` → 「Scanning the LAN for Fleety servers… (3s)」→ 編號清單（例：`1. mini  ws://192.168.1.10:8787`，已存 profile 的加 `(saved)`）→ 輸入 `1` → 「Using 'mini' (ws://…) as the current server.」→ 「Pairing code (Enter to skip): 」→ 輸入碼 → 「Paired; 'mini' is now your current server.」
- 掃描無結果：印「No Fleety server found on this network.」加現行 usage 提示（含 ws:// 範例與 pair 說明）。
- 非 TTY 或 FLEETY_MDNS_DISABLED：直接現行 usage 提示，不掃描。
- `fleety init ws://…` 與 `fleety pair <code>`：行為與訊息完全不變。

**介面與資料形狀：**

- `struct DiscoveredServer { name: String, url: String }`（CLI 內部）；`discover_all_via_mdns(window: Duration) -> Vec<DiscoveredServer>`（URL 去重、發現順序）。
- 純函式（可單測）：instance 名→顯示名（剝 `fleety-` 前綴、空退 URL）；清單渲染行；選擇輸入解析（`&str` → `Selection::Pick(usize) | Cancel | Invalid`，1-based 邊界檢查）。
- profile 建立與配對重用既有 upsert_profile_and_use / pair，簽名不動。

**失敗模式：**

- mDNS daemon 建立失敗 → 視同無結果（usage 提示），不 panic。
- 選擇輸入非法（非數字、超界）→ 重新提示；EOF/空行 → 取消。
- 配對碼錯誤 → 既有 pair 錯誤訊息（可重跑 `fleety pair`），profile 保留。

**驗收準則：**

- cargo test -p fleety-cli：顯示名剝前綴（含缺名退 URL）、選擇解析（合法/超界/非數字/空/EOF 語義）、清單渲染含 saved 標記、URL 去重的收集邏輯（以注入事件序列或純函式切分測試）。
- 顯式 init/pair 的既有測試不回歸；cargo clippy -D warnings、fmt 乾淨。
- 互動端到端（掃描實網）維持專案手動驗證 posture：發版後在 Windows CLI 對 Mac mini 實跑一次。

**範圍邊界：**

- 範圍內：crates/fleety-cli/src/main.rs、docs/env.md、README.md。
- 範圍外：resolver、server mdns.rs、fleety pair、connections.toml 格式。

## Risks / Trade-offs

- [3 秒窗口收不齊慢速廣播] → 單台情境 resolver fallback 仍兜底；清單漏台重跑 init 即可；窗口常數集中一處便於調整。
- [同網段惡意廣播假 server] → 與現行隱式 fallback 同級風險；配對碼仍是信任門檻（token 只在配對成功後寫入），指紋 pin 機制在既有 enrolled 流程不動。
- [行輸入在極簡終端的相容性] → 純 stdin 行讀，比 ratatui 相容面更廣。

## Migration Plan

單版出貨，無資料遷移。回滾 revert 即可。

## Open Questions

- 無阻斷項。
