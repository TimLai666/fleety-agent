## Context

provider 在 crates/fleety-server/src/providers.rs 寫死成 ProviderTiers { main, cheap },各一個 Arc<dyn ModelProvider>,由 build(prefix) 從扁平 env(FLEETY_MODEL_* / FLEETY_CHEAP_MODEL_*)建構;resolve(tier) 只認 "cheap" 與 else→main。subagent 的 SpawnRequest.tier 也走 resolve。剛 archive 的 resilient-model-calls 在 crates/agent-core/src/retry.rs 提供 429/5xx classify + 退避,且每個 provider(openai/gemini)內部已套用。ModelProvider trait 在 agent-core,工作區規則禁止 agent-core 依賴任何 fleety crate。fleety-tools 已有 toml 解析(config.rs);fleety-server 依賴 fleety-tools。

## Goals / Non-Goals

**Goals:**

- 可配置任意多個同類型 provider 實例(多帳號 / 多端點),以具名方式定義。
- 群組可 round_robin 分散額度,或 failover 主備;成員出錯就換下一個。
- 不改 agent-core 的 ModelProvider trait;多帳號邏輯收在 server 端。
- 沒有 providers.toml 時,既有 env 行為完全不變。

**Non-Goals:**

- 不做 config 指令 / 互動式管理 providers.toml(屬後續 provider-config-surface)。
- 不做壞帳號冷卻 / 健康追蹤 / 權重 / group 內異質能力合併 / 只在特定錯誤碼才轉移(列 Open Questions)。
- 不改 wire 協定、不引入重型依賴。

## Decisions

### providers.toml 獨立檔與 schema

新增 ~/.fleety/providers.toml(路徑可由 FLEETY_PROVIDERS 覆寫),與 config.toml 分開(金鑰隔離)。`[[provider]]`:name(必)、base_url、model、key、stream(bool)、modalities(字串,如 "text,image")、effort(low/medium/high)。`[[group]]`:name、members(provider 名稱陣列)、strategy("round_robin"|"failover")。`[roles]` 表:main/cheap/任意名 → provider 或 group 名。解析成純資料結構(ProvidersConfig),用既有 toml 解析;解析失敗 fail-soft 成「無 providers.toml」(退回 env),絕不崩潰。理由:結構化檔撐得起 N 個;獨立檔便於權限與金鑰隔離;fail-soft 不讓壞檔擋掉啟動。

### PoolProvider 包裝 provider(agent-core 不動)

在 crates/fleety-server/src/pool.rs 新增 PoolProvider { members: Vec<Arc<dyn ModelProvider>>, strategy: Strategy, next: AtomicUsize },實作 agent-core 的 ModelProvider。complete/complete_streaming 依策略決定成員嘗試順序,逐一呼叫直到成功;對外仍是一個 ModelProvider。理由:把多帳號/輪替/換手邏輯收在 server 端的一個 adapter,agent-core trait 與 run loop 完全不動。

### 任何錯換下一個成員;round_robin vs failover(純函式選順序)

成員 provider 內部已含 resilient-model-calls 的退避重試;PoolProvider **不重複重試**,而是:呼叫成員 i,若回 Err 就換下一個成員,全部成員都失敗才回**最後一個**錯。任何錯都換(不分辨錯誤碼:成員已把錯收斂成 CoreError::Provider 字串難以分辨,且換到同型成員最壞只是再次失敗,無害)。`round_robin`:每次呼叫以 AtomicUsize fetch_add 決定起點,成員依序輪替(分散額度);`failover`:固定從 0 起、壞了才往後(主備)。「給定起點與成員數,產生嘗試順序」抽成純函式 attempt_order(start, len) 以利測試。理由:把難測的 HTTP 隔離,順序邏輯可窮舉測試;成員內部已處理暫時性重試,pool 只負責跨帳號換手。

### resolve(name) 查具名池 + env fallback(零設定相容)

ProviderTiers 改為持有「名稱 → Arc<dyn ModelProvider>」的具名表(單一 provider 或 group 包成 PoolProvider 都是 Arc<dyn>)+ roles 映射。resolve(role_or_name):先查 roles 映射 → 名稱 → provider;名稱不存在 → main;providers.toml 不存在或為空 → 用既有 build("FLEETY_MODEL")/build("FLEETY_CHEAP_MODEL") 建出 main/cheap(現況行為)。理由:相容優先,零設定不變;subagent tier 自然支援任意名稱。

### 成員同質能力假設

PoolProvider 假設一個 group 的成員是同一種模型的多帳號/端點(同 capabilities）：capabilities() 回傳 member[0] 的能力;with_effort(e) 回傳「每個成員都套用 e」的新 PoolProvider。理由:多帳號通常是同模型;異質合併複雜度高,列後續。

## Implementation Contract

**行為(Behavior):**

- 有 providers.toml:依其建立具名 provider 與 group;角色/tier 名解析到對應 provider 或 pool。
- group strategy=round_robin:連續呼叫輪流落在不同成員(分散)。strategy=failover:固定打第一個成員,該成員失敗才打第二個。
- 任一成員呼叫成功 → 回其結果;成員失敗 → 換下一個;全部失敗 → 回最後一個錯誤(CoreError::Provider),不 panic。
- 無 providers.toml(或解析失敗):行為與現況完全相同(env 建 main/cheap)。
- 未知角色/tier 名 → main。

**介面 / 資料形狀:**

- providers.toml:`[[provider]]`(name/base_url/model/key/stream/modalities/effort)、`[[group]]`(name/members/strategy)、`[roles]`(name→name)。
- crates/fleety-server/src/pool.rs:`enum Strategy { RoundRobin, Failover }`;`fn attempt_order(start: usize, len: usize) -> Vec<usize>`(純,起點開始繞一圈);`struct PoolProvider { members, strategy, next: AtomicUsize }` impl ModelProvider(complete/complete_streaming/capabilities/with_effort)。
- ProvidersConfig 純資料結構 + `fn parse(text: &str) -> Result<ProvidersConfig>`(純,可測);載入器讀檔 fail-soft。
- providers.rs:ProviderTiers 改具名表 + roles;resolve(name) 如上;保留 env fallback。

**失敗模式:**

- providers.toml 不存在 → env fallback。解析錯誤 → 記 log + 當作不存在(env fallback),不崩潰。
- group members 名稱不存在 → 略過該成員(記 log);group 空 → 視為未定義(回退 main)。
- 成員全失敗 → 回最後錯誤。

**驗收標準(Acceptance):**

- 單元測試:parse 解析 [[provider]]/[[group]]/[roles];壞 toml → Err(載入器 fail-soft 測試另測)。
- 單元測試:attempt_order(start,len) 對 round_robin(起點輪替繞一圈)與邊界(len=1、start≥len)。
- 單元測試:PoolProvider 以可注入的假成員(scripted ModelProvider:前 N 個回 Err、之後成功)驗證「換下一個成員」直到成功;全失敗回最後錯;round_robin 連續呼叫起點遞增;capabilities()=member[0]。
- 既有 providers/resolve 測試全綠(無 providers.toml 時 main/cheap 行為不變)。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒;真 HTTP 多帳號往返為手動驗證。

**範圍邊界:**

- In scope:providers.toml schema+解析+載入、PoolProvider(round_robin/failover、任何錯換手)、resolve 具名+env fallback、成員同質能力、文件。
- Out of scope:config 指令/互動 UI(provider-config-surface)、冷卻/健康/權重/異質合併/錯誤碼選擇性轉移、agent-core trait 變更。

## Risks / Trade-offs

- [任何錯都換手可能對 4xx(請求本身錯)也換,浪費一輪] → 換到同型成員最壞再錯一次即回,無資料風險;只多一次呼叫。選擇性轉移列後續。
- [round_robin 把同一對話的連續呼叫打到不同帳號,可能影響供應商側的快取/連續性] → 可接受(多帳號本就為分散);需要黏著時用 failover。
- [providers.toml 含金鑰] → 獨立檔 + 沿用既有 secret 慣例;不記入一般日誌。
- [解析失敗靜默退回 env 可能讓使用者誤以為 providers.toml 生效] → 解析錯誤明確 log 警告。

## Migration Plan

- 純加層:無 providers.toml → 走既有 env(main/cheap),完全相容。要用多帳號就放 providers.toml。
- 無資料遷移。回滾:刪除/清空 providers.toml 即回到 env 行為。

## Open Questions

- 壞帳號冷卻 / 健康追蹤 / 權重輪替:MVP 不做。
- group 內異質模型能力的合併(而非取 member[0]):MVP 不做。
- 只在特定錯誤碼(如 429)才換手、其他錯不換:MVP「任何錯都換」;之後可細化。
- providers.toml 的 config 指令與互動式管理:屬 provider-config-surface(下一個 change)。
