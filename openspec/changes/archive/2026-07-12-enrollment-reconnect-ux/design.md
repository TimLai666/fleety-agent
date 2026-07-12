## Context

配對碼鑄造:`AuthStore::create_pairing()` 生一組短效碼並持久化到 auth 狀態檔——但那是 server 行程內的方法,現有觸發只有對話裡的 `pair_create` 工具。沒有客戶端命令能請 server 現生一組。分離行程直接呼叫 create_pairing 會寫檔但不進 running server 的記憶體 pairing map,redeem 因而失敗;所以正確做法是請「執行中的 server」鑄造。loopback 信任(已出貨)讓本機 CLI 免配對連上,正好成為請求鑄造的通道。

TUI 連線:`run_tui` 連上、送 Hello、進事件迴圈讀 frame。server 若以 `unauthenticated` 拒絕,送一個 Error frame 後關閉。目前 Error 處理把任何 Error 當「turn 出錯」只改狀態列;連線關閉走 `None` 分支進 `reconnect`(8 次退避),每次「重連成功」又立刻收到同樣的 Error+關閉,於是不斷重來,使用者只看到狀態列在「reconnecting…」與「agent error」間閃動,從不被告知要配對。

## Goals / Non-Goals

**Goals:**
- server 主機一行 `fleety pair-code` 生配對碼給別的裝置用。
- TUI 遇認證拒絕即以可讀訊息終止,不空轉。

**Non-Goals:**
- 不改配對碼 TTL、格式、或 redeem 流程。
- 不改暫時斷線的重連行為(只有 unauthenticated 終止)。
- 不做 server-cli(`fleety-server pair-code`);由裝在 server 主機的 `fleety` 經 loopback 信任達成(已由 local-server-trust 出貨)。

## Decisions

### 決策一:MintPairingCode 請求/回覆 frame

新增 `ClientMsg::MintPairingCode`(無欄位)與 `ServerMsg::PairingCode { code: Option<String>, error: Option<WireError> }`。server 在 session 迴圈處理:`auth.required()` 為真 → `create_pairing()` 鑄造、回 `code`;為假 → 回 error(認證關閉,配對碼不被使用,說明如何開認證)。能到達 session 的連線必已過 Hello 認證或 loopback 信任,故無需額外權限參數;隨機未認證 LAN 連線在 Hello 已被拒。

- 否決「分離行程 `fleety-server pair-code` 直接呼叫 create_pairing 寫檔」:寫檔的碼不在 running server 的記憶體 map,redeem 會失敗。必須請執行中的 server 鑄造。

### 決策二:`fleety pair-code` 命令

`fleety pair-code`:走既有 `connect_hello`(解析目前 current profile、loopback 信任或 token 認證連上、收 Welcome),送 `MintPairingCode`,收 `PairingCode`:有 code 印「Pairing code: <code>\nOn the other device: fleety pair <code> (expires soon)」;有 error 以可讀訊息回報;收到 unsupported(舊 server)給版本提示。

### 決策三:TUI 對 unauthenticated 終止

TUI 的 Error 處理:`error.kind == "unauthenticated"` → 設狀態「Not paired with this server — run `fleety pair <code>` (mint one with `fleety pair-code` on the server host), then reopen the TUI.」並 `should_quit = true`,不進重連。其餘 Error 維持現行(清 in-flight、顯示訊息、可能後續斷線重連)。此判斷在主迴圈 Error 分支,涵蓋首次連線與重連後——重連本身不需改(重連後仍會收到 Error frame 由此處終止)。判斷抽為純函式(Error kind → 是否為認證終止)。

## Implementation Contract

**行為(操作者視角):**
- server 主機 `fleety pair-code` → 印一組短效配對碼(＋在別台 `fleety pair <code>` 的提示)。認證關閉的 server → 說明配對碼未被使用、如何開認證。舊 server → 版本提示。
- 沒配對就開 `fleety tui` → 立即顯示「尚未配對,請 fleety pair …」並退出,不再無限重連。
- 暫時斷線(非認證) → 重連行為完全不變。

**介面與資料形狀:**
- `ClientMsg::MintPairingCode`;`ServerMsg::PairingCode { code: Option<String>, error: Option<WireError> }`(serde,additive)。
- `fleety pair-code` 子命令(無旗標)。
- 純函式:Error kind → 認證終止判定(`is_auth_rejection(&str) -> bool`,命名 apply 時定)。

**失敗模式:**
- 連不上/未配對遠端 → `connect_hello` 既有錯誤與 remediation。
- 舊 server 回 unsupported/未知 frame → pair-code 印版本提示。
- 認證關閉 → PairingCode.error 說明。

**驗收準則:**
- cargo test:MintPairingCode/PairingCode round-trip(protocol,additive、舊形容忍);server 鑄造分支(auth 開回 code、auth 關回 error)——以 authenticate/AuthStore 既有測試風格覆蓋鑄造函式;is_auth_rejection 純函式(unauthenticated → true、其他 → false)。
- 既有 TUI 與 auth 測試不回歸。
- 全 workspace test/clippy/fmt 乾淨。
- 端到端(發版後人工):server 主機 `fleety pair-code` 生碼、另一台 `fleety pair <code>` 成功;未配對開 TUI 見終止訊息。

**範圍邊界:**
- 範圍內:crates/fleety-protocol/src/lib.rs、crates/fleety-server/src/conn.rs、crates/fleety-cli/src/main.rs。
- 範圍外:loopback 信任(已出貨)、pair_create 工具、配對碼 TTL/格式、docs 大改(命令列表順帶補一行)。

## Risks / Trade-offs

- [已認證的遠端裝置也能 `fleety pair-code` 鑄碼] → 與既有 `pair_create` 工具同等權限(已配對裝置本就能生碼配新裝置),非新增暴露面。
- [TUI 終止 vs 重連的判定僅看 Error kind] → kind 是 server 明確設的 `unauthenticated`,穩定;其他 kind 保守走原路(重連),不誤終止。

## Migration Plan

單版出貨,無資料遷移。回滾 revert。

## Open Questions

- 無阻斷項。
