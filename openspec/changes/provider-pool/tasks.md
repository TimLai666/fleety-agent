## 1. providers.toml schema 與解析(純函式)

- [x] 1.1 在 crates/fleety-tools 新增 providers.toml 的資料結構與 `parse(text) -> Result<ProvidersConfig>`(純函式,用既有 toml):`[[provider]]`(name/base_url/model/key/stream/modalities/effort)、`[[group]]`(name/members/strategy)、`[roles]`(name→name),交付 "Named providers and groups are defined in a separate file" 的解析面;對應設計「providers.toml 獨立檔與 schema」。先寫失敗測試:解析含兩 provider + 一個 round_robin group + main→group 的 toml;壞 toml → Err。
- [x] 1.2 新增載入器:從 `~/.fleety/providers.toml`(FLEETY_PROVIDERS 可覆寫)讀檔並 `parse`,檔案不存在或解析失敗 → 回 None(fail-soft,記 log 警告),交付 "Named providers and groups are defined in a separate file" 的 fail-soft 面;對應設計「providers.toml 獨立檔與 schema」。驗證:不存在 → None;壞檔 → None 且不 panic(單元測試,FLEETY_PROVIDERS 指向暫存檔)。

## 2. PoolProvider(輪替 / 故障轉移)

- [x] 2.1 在 crates/fleety-server/src/pool.rs 實作純函式 `attempt_order(start, len) -> Vec<usize>`(從 start 起繞一圈),交付 "A group pools members with round-robin or failover" 的順序面;對應設計「任何錯換下一個成員;round_robin vs failover(純函式選順序)」。先寫失敗測試:用 spec example 表(round_robin call1/call2、failover)與邊界(len=1、start≥len)。
- [x] 2.2 在 pool.rs 實作 `PoolProvider { members, strategy, next: AtomicUsize }` impl agent-core `ModelProvider`:complete/complete_streaming 依 `attempt_order` 逐一呼叫成員、任一 Err 換下一個、全失敗回最後錯(不 panic);round_robin 每次呼叫原子遞增起點;capabilities() 取 member[0];with_effort() 回各成員套 effort 的新 pool,交付 "A group pools members with round-robin or failover" 與 "A pooled provider reports homogeneous capabilities";對應設計「PoolProvider 包裝 provider(agent-core 不動)」與「成員同質能力假設」。先寫失敗測試:以可注入的 scripted ModelProvider(前 N 回 Err、之後成功)驗證換手到成功、全失敗回最後錯、round_robin 起點遞增、capabilities=member[0]。

## 3. 具名解析 + env fallback

- [x] 3.1 在 crates/fleety-server/src/providers.rs 把 ProviderTiers 改為具名表(name→Arc<dyn ModelProvider>,group 包成 PoolProvider)+ roles 映射,由 providers.toml(經 #1)建構;providers.toml 不存在/空 → 既有 build("FLEETY_MODEL")/build("FLEETY_CHEAP_MODEL") 建 main/cheap,交付 "Roles resolve by name with an env fallback" 的建構/相容面;對應設計「resolve(name) 查具名池 + env fallback(零設定相容)」。先寫失敗測試:無 providers.toml → resolve("cheap")/("main") 行為與現況相同(沿用既有測試)。
- [x] 3.2 `resolve(role_or_name)` 先查 roles 映射 → 名稱 → provider,未知名 → main;subagent tier 可指任意 provider/group 名,交付 "Roles resolve by name with an env fallback" 的解析面與「subagent tier 指 pool」;對應設計「resolve(name) 查具名池 + env fallback(零設定相容)」。驗證:providers.toml 定義 group 後 resolve("該group名") 回 pool;未知名回 main(單元/整合測試)。

## 4. 文件

- [x] 4.1 [P] 在 docs/env.md 記錄 providers.toml 格式([[provider]]/[[group]]/[roles] 欄位與 strategy)、FLEETY_PROVIDERS 路徑覆寫、與 env(FLEETY_MODEL_*/FLEETY_CHEAP_MODEL_*)的 fallback 關係,交付 "Named providers and groups are defined in a separate file" 的文件面。驗證:內容審查涵蓋 schema、strategy 兩值、env fallback 與「任何錯換下一個」語意。
