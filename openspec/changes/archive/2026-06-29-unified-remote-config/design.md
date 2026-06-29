## Context

`fleety_tools::config`(crates/fleety-tools/src/config.rs)是三 binary 共用的設定層:典型登錄表(扁平 FLEETY_* 鍵)+ parse/run(list/get/set/unset)+ providers.toml 的 provider/group/role 子指令(run_providers_at(path, args))。全是改本機檔(config_path()/providers_path()),不走網路。協定(crates/fleety-protocol/src/lib.rs)已有 device 定址的 request/result 成對訊息:AuditList{device_id}→AuditListResult、RollbackList→RollbackListResult、ServerStatus→ServerStatusResult;這些在 server 端讀 server-local 資料。真正路由到 daemon 的原語是 RunTool dispatch + waiter(conn.rs 約 837 行「route to the waiter」;daemon main.rs 約 576 行處理 ServerMsg::RunTool→ToolResult)。連線認證:Hello 帶 token + FLEETY_REQUIRE_AUTH(registry)+ pairing。provider 建構:ProviderTiers::from_env() 在每條連線建立時(build_connection_stack)跑,且 load() 每次重讀 providers.toml;但扁平鍵是開機時 seed_env_from_config 一次性塞進 env,且 resolve() 是 env 優先。

## Goals / Non-Goals

**Goals:**

- 從 CLI 經已認證連線管理**連到的 server**的設定(扁平鍵 + provider/group/role),不必 SSH 到 server。
- 重用既有 fleety_tools::config 邏輯,不在協定層複製。
- 對套用時機誠實:回報變更何時生效(下次連線 vs 需重啟),不假裝即時熱抽換。
- 保留本機 fleety-server config 作 bootstrap。

**Non-Goals:**

- 裝置端設定(`--target <device-id>` 路由到 daemon)——後續 change `remote-config-devices`,重用 RunTool dispatch。
- 連線中(mid-session)即時熱抽換 provider——MVP 不做(provider 綁在連線生命週期)。
- TLS 自動配置(只在文件建議)、admin/RBAC 角色、config 變更的版本歷史/diff/回滾(audit 已記錄)。

## Decisions

### 協定:Config* request/result(對齊既有 device 定址模式)

fleety-protocol 新增**單一** ClientMsg `ConfigExec { target, args: Vec<String> }`(`args` 是完整的 config 引數向量,直接餵既有 `parse`/`run` 與 `parse_providers`,涵蓋 list/get/set/unset 與 provider/group/role 動詞)+ ServerMsg `ConfigResult { ok, output, effect, error }`(errors-as-messages,WireError)。`target` enum:`Server` | `Local` | `Device(String)`(`Local` 由 CLI 端處理、不會上線;`Device` 型別先保留,本案 server 回 unsupported)。理由:照搬 AuditList/RollbackList 成對請求/回應;args 直餵既有 parser=不重造指令文法,協定面最小(一個 exec 勝過四個半對稱變體——provider/group/role 本就有子動詞,塞進 args 最自然)。

### server handler:認證閘 + 在 server host 重用既有 config 函式

conn.rs 處理 Config* 時:先過認證(連線未認證且 FLEETY_REQUIRE_AUTH → 回 unauthenticated WireError);target=Server → 在 server 的 config_path()/providers_path() 上呼叫既有 config::run 等價邏輯(list/get/set/unset)或 run_providers_at(providers_path, args),擷取其輸出與錯誤包成 ConfigResult;每次 mutate 寫 audit。理由:邏輯零複製、與本機 config 完全同行為;認證閘是唯一新增的安全邊界。

### target 定址:server(預設)/ local;device 留後續

CLI 端:`fleety config [--target server|local] <...>`。target=Server(預設)→ 連線送 Config* 給 server;target=Local → 不連線,維持現有「改 CLI 本機 ~/.fleety」路徑(等同今天的 fleety config)。target=Device 在 CLI 解析得到,但本案送出會由 server 回 unsupported（或 CLI 直接擋並提示後續 change）。理由:server 是核心痛點;local 是既有行為的保留;device 拆出去(要動 daemon、第四子系統)。

### 套用語意:寫檔即持久化,生效時機誠實回報

mutate 一定**寫檔持久化**。生效時機由純函式 `config_effect(args) -> Option<ConfigEffect>` 依**動詞**判定(fleety-tools 不依賴 fleety-protocol,故用本地 `enum ConfigEffect { NextConnection, Restart }`,server 端映射到 protocol `Effect`):
- mutating 的 provider/group/role(`providers.toml`)→ `NextConnection`(ProviderTiers::from_env 每條新連線重讀 providers.toml)。
- 扁平 `set`/`unset`(`config.toml`)→ `Restart`(開機才 seed_env_from_config,且 env 優先,既有 env 不會被新值蓋過——含 `FLEETY_MODEL` 這類扁平鍵也是 Restart)。
- 讀取(`list`/`get`、`provider list`)→ `None`(無變更)。
MVP **不做** mid-session 熱抽換。理由:對現有 provider 生命週期誠實,避免「設了卻沒生效」的假象;判定抽純函式可測。注意「設模型」要走 provider 池(`provider`/`role`)才是下次連線生效;扁平 `FLEETY_MODEL` 需重啟。

### CLI 線路與既有指令相容

fleety-cli 的 config 進入點解析 `--target`;target=local 或無連線設定時走本機路徑(回歸不變),否則建立(認證)連線、送 Config*、印 ConfigResult.output 與 effect 提示。fleety config provider edit 的互動畫面本案維持本機(遠端互動編輯列後續)。

## Implementation Contract

**行為(Behavior):**

- `fleety config set FLEETY_MODEL gpt-5`(預設 target=server)→ 連線送 ConfigSet → server 寫自己的 config.toml → 回 ConfigResult{ ok, effect: Restart }，CLI 印「已設定,需重啟 server 生效」。
- `fleety config provider add … `（target=server)→ server 寫 providers.toml → ConfigResult{ ok, effect: NextConnection }。
- `fleety config list`（target=server)→ 回 server 解析後的設定列表(secret 遮罩,沿用 display_value)。
- `fleety config --target local set …` → 不連線,改 CLI 本機檔(等同今天行為)。
- 未認證連線在 FLEETY_REQUIRE_AUTH 下送 Config* → ConfigResult 帶 unauthenticated 錯誤,不改任何檔。
- 任一 config 錯誤(未知鍵、provider 重名、懸空引用…)→ ConfigResult.error 帶訊息,server 不崩潰、檔案不變。

**介面 / 資料形狀:**

- fleety-protocol:`ClientMsg::ConfigExec { target: ConfigTarget, args: Vec<String> }`;`ServerMsg::ConfigResult { ok: bool, output: String, effect: Option<Effect>, error: Option<WireError> }`;`enum ConfigTarget { Server, Local, Device(String) }`;`enum Effect { NextConnection, Restart }`(serde,wire-stable)。
- fleety-server/conn.rs:Config* 分支 → auth 檢查 → 在 server 路徑跑既有 config 邏輯 → 包 ConfigResult;mutate 寫 audit。
- fleety-tools/config.rs:純函式 `config_effect(args: &[String]) -> Option<ConfigEffect>`(本地 `enum ConfigEffect { NextConnection, Restart }`;mutating provider/group/role → NextConnection、`set`/`unset` → Restart、讀取 → None),供 server 映射到 protocol `Effect`;既有 run/run_providers_at 不變。
- fleety-cli:config 進入點解析 --target;Server → 送訊息收 ConfigResult;Local → 既有本機路徑。

**失敗模式:**

- 連線失敗 / server 不可達 → CLI 回明確錯誤,建議 --target local 或到 server host 用 fleety-server config。
- 未認證 → ConfigResult unauthenticated,不改檔。
- target=Device → 本案回 unsupported,提示為後續 change。
- config 邏輯錯誤 → ConfigResult.error,檔案不變、不 panic。

**驗收標準(Acceptance):**

- 單元測試:`config_effect` 對 provider/group/role 與模型相關鍵回 NextConnection、其餘扁平鍵回 Restart(表格驅動)。
- 單元測試:Config* / ConfigResult / ConfigTarget / Effect 的 serde round-trip(wire 相容,沿用既有協定測試樣式)。
- 單元測試:server 端 Config* handler 在臨時 config/providers 路徑上 set→list 反映變更、未知鍵→error 且檔案不變;未認證(模擬 require_auth)→ unauthenticated 不改檔。
- 既有 fleety config(本機)回歸不變;clippy -D 乾淨、agent-core host-free、env 測試單執行緒。真連線往返(CLI↔server)手動驗證。

**範圍邊界:**

- In scope:協定 Config* 訊息、server handler（auth + 本機執行 + effect 標註 + audit）、CLI --target(server/local)、config_effect 純函式、文件。
- Out of scope:target=device(daemon 路由,後續 change)、mid-session 熱抽換、TLS 自動化、RBAC、config 版本史。

## Risks / Trade-offs

- [扁平鍵改了要重啟才生效,體感不如即時] → effect 欄位誠實回報,文件說明;providers.toml 路徑(含模型池)下次連線就生效,涵蓋主要使用情境(設模型)。日後可做 mid-session 熱抽換(Open Question)。
- [模型 key/token 過線] → 強制走已認證連線(FLEETY_REQUIRE_AUTH + pairing),文件建議遠端用 TLS;不認證的本機 loopback 連線維持現況風險等級。
- [args 直餵既有 parser] → 既有 parse/parse_providers 已是純函式且驗證完整,直餵不擴大攻擊面;server 端仍過 auth 閘。
- [target=device 先擋掉可能讓人以為支援] → CLI 與 ConfigResult 都明確回「後續 change」,不靜默吞。

## Migration Plan

- 純加層:不送 Config* 的舊 CLI、或 --target local,行為完全不變;fleety-server config 仍可用。
- 無資料遷移。回滾:移除 Config* 處理,CLI 退回本機-only。

## Open Questions

- 裝置端設定(`--target <device-id>` 路由到 daemon,重用 RunTool dispatch + waiter)：後續 change `remote-config-devices`。
- mid-session 熱抽換 provider(改了模型當前連線即生效,不必重連):需把 provider 變成連線可換的 handle,較大,後續。
- 扁平鍵在不重啟下生效(繞過 boot-time env seed 的 env 優先):需重新設計 seed/precedence,後續。
- TLS 自動配置、admin/RBAC、config 變更版本史/diff:皆後續。
