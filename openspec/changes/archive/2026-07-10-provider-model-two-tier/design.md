## Context

現行 provider/model 的概念錯位與 `PoolProvider::capabilities()` 同質假設,已在 proposal 與 docs/design-cli-config.md §1.2/§3.3/§9 與決策 M2/M5 說明。相關程式碼(讀過驗證):`fleety-tools` 的 `providers_config.rs`(`ProviderSpec { name, base_url, model, key, stream, modalities, effort, auth }` + `GroupSpec` + `roles: BTreeMap<String,String>`;`parse`/`load_from`/`load_or_default`/`validate`/`write_providers`);`fleety-server` 的 `providers.rs`(`build_provider(ProviderBuild)`、`build_from_spec`、`ProviderTiers::from_config` 建 provider + group→`PoolProvider` + roles、`build_main` 環境 echo fallback);`pool.rs`(`PoolProvider::capabilities()` 取 `members.first()`);`conn.rs` 用 `provider.capabilities().audio` 填 `Welcome.audio_input`;`config.rs` 的 `ProviderCmd`/`parse_providers`/`run_providers_at`;CLI `provider_tui.rs` 互動編輯器。關鍵事實:附件原生 vs 降級已在**每個 provider 自己的 `complete()`**(`openai.rs::wire_content` 用各自 `caps`)完成,故「依實際 member 動態決定」在 complete 層已成立;pool 的聚合 `capabilities()` 只被 `Welcome.audio_input` 這類提示消費。

## Goals / Non-Goals

**Goals:**
- provider 與 model 兩層清楚分離:endpoint/secret/auth 屬 provider;model 名與 stream/modalities/effort 屬 member。
- 一個 provider 可被不同 role 用不同 model;role 固定 main/cheap,各是一個 pool。
- 混族 pool 能力正確(聯集,不 first/交集);參照完整性寫入前擋。
- providers.toml 舊→新去重遷移;扁平 env 保留為 bootstrap seed;壞結構化設定硬錯不退 echo。

**Non-Goals:**(見 proposal)認證預設開、本機 config scope 過濾、遠端面板/protocol、傳輸 wss、裝置分級、加密。

## Decisions

### Provider 的 type-tagged enum 資料模型

`providers.toml` 的 `[providers.<name>]` 帶 `type`。`type="api"`:必填 `base_url`、可有 `key`(secret)、禁 oauth token。`type="oauth:codex"`:token 由 `fleety auth login <name>` 產(per-provider,現行 codex-responses 走 oauth token store),禁 `base_url`/`key`。序列化以 `type` 當 tagged discriminator;`type` 未知時解析回明確錯誤但**列出已知 type**,新增別種 oauth 型只加註冊項不改核心分支。資料型別放 `providers_config.rs`,`fleety-server` 消費。

### Model role 兩層 pool 與 member 屬性下沉

新增 `[models.main]` / `[models.cheap]`,各有 `strategy`(`single`|`round_robin`|`failover`,沿用現行 `Strategy` enum + 補 `single`)與 `members`。`Member { provider: String, model: String, stream: bool, modalities: Option<String>, effort: Option<String> }`。runtime:`ProviderTiers::from_config` 先建 provider 連線參數表,再對每個 role 用其 members 建一個 `PoolProvider`——每個 member 經 `build_provider` 組(provider 的 `base_url`/`key`/`auth` + member 的 `model`/`stream`/`modalities`/`effort`);`resolve("main"|"cheap")` 回該 pool,未知 selector 回 main。取代舊 `providers/groups/roles` 三段。

### 混族 pool 的動態能力(pool capabilities = member union)

`PoolProvider::capabilities()` 從 `members.first()` 改為**跨 members 聯集**(任一 member 支援某模態 → pool 報支援)。理由:附件降級已在各 member 的 `complete()` 內按自身 caps 做,pool 的聚合能力只用於 `Welcome.audio_input` 這種「要不要送」提示;聯集讓有能力的 member 收到原生附件、無能力的自行降級,絕不 first/交集(推翻同質假設,對應 M2)。

### 參照完整性 validate(寫入前,非 runtime fail-soft)

`validate(cfg)`:每個 `member.provider` 必須是已定義 provider(否則報 undefined provider 名);`strategy="single"` 的 role members 必須恰一個;api provider 缺 `base_url` 或帶 token、oauth provider 帶 `base_url`/`key` 皆拒。`write_providers` 寫前跑 validate,壞設定不落檔;刪 provider 前若被任何 role member 引用則拒(或提示先改 role)。runtime 的 `load_from` 仍 fail-soft(壞檔退 env tier),但**寫入路徑不 fail-soft**。

### providers.toml 一次性去重遷移

`migrate_providers(old) -> new`:把「`base_url`+`key`(+`auth`)相同、只差 `model`」的舊 provider **合併成單一 provider**(名取一致規則,如首個或 host 衍生),各自的 `model`/`stream`/`modalities`/`effort` 成為 member;舊 `roles` (main/cheap→provider/group) 對應到新 `models.<role>` 的 members(group→多 member pool,單 provider→單 member）。搬不動的欄位(衝突、無對應 role)**明確 warn 列出**,不默默吞。遷移冪等:已是新格式(有 `[models.*]`)則跳過。

### FLEETY_MODEL_* bootstrap seed 與壞設定硬啟動錯誤

無結構化 provider/model(providers.toml 缺或空)時,`FLEETY_MODEL_*` / `FLEETY_CHEAP_MODEL_*` 自動組 `models.main`(+cheap):headless/CI/Docker 三行 env 照跑。若 providers.toml **存在但結構化設定壞/參照不完整**,改**硬啟動錯誤**(server 拒開機、回明確訊息),不再靜默退 echo;echo 佔位僅保留給「完全無設定」以維持連線可驗證。

### provider/model 命令與互動編輯器改新模型

`config provider add <name> --type api --base-url … [--key …]` / `--type oauth:codex`;`provider set/remove/list`(list 依 type 顯示不同欄位、mask key)。`config model set <main|cheap> --member <provider>/<model> [--stream][--modalities …][--effort …] [--member …] --strategy <single|round_robin|failover>`;`model show/unset`。CLI `provider_tui.rs` 互動編輯器改對應新結構(provider 依 type 顯欄位、model role 編輯 member 清單)。

## Implementation Contract

**行為:**
- `config provider add openai1 --type api --base-url https://api.openai.com/v1 --key sk-x` 後 `config model set main --member openai1/gpt-4o --member openai1/gpt-4o-mini --strategy failover`;server 起動後 `main` role 為 failover pool、附件送到有能力的 member。
- `config provider add codex1 --type oauth:codex` 後 `fleety auth login codex1` 產 token;codex1 provider 禁 base_url/key。
- 舊 providers.toml(provider 綁 model + group + role)首次讀取後自動遷移為 `[providers.*]` + `[models.*]`,重複 provider 去重,搬不動的欄位有 warn。
- 未設結構化設定時 `FLEETY_MODEL_*` 三行 env 仍組出 main;結構化設定壞則 server 拒開機並回明確錯誤。
- 混族 pool 的 `audio_input`(Welcome)為 members 聯集。

**介面 / 資料形狀:**
- `providers.toml`:`[providers.<name>] type=…`(api:base_url/key;oauth:codex:無);`[models.<role>] strategy=…` + `members=[{provider,model,stream,modalities,effort}]`。
- `providers_config.rs` 公開:`Provider`(tagged enum)、`ModelPool { strategy, members }`、`Member`、`ProvidersConfig { providers: BTreeMap<String,Provider>, models: BTreeMap<String,ModelPool> }`、`parse`/`load_from`/`load_or_default`/`validate`/`write_providers`/`migrate_providers`。
- `pool.rs`:`capabilities()` 回 members 聯集。

**失敗模式:**
- 未知 provider `type` → 解析錯誤並列已知 type。
- member 參照未定義 provider / `single` 非恰一 member / api 缺 base_url / oauth 帶 base_url|key → validate 拒(寫入前,不落檔)。
- 刪被 role 引用的 provider → 拒並提示。
- providers.toml 壞或結構化設定不完整 → server 硬啟動錯誤(非 echo)。
- 遷移搬不動欄位 → warn 列出,不吞。

**驗收條件:**
- fleety-tools 單元:tagged provider round-trip、validate 各拒項、migrate 去重 + 屬性下沉 + 冪等 + warn。
- fleety-server 單元:from_config 建 main/cheap member pool、resolve、pool `capabilities()` 聯集(推翻 first)、bootstrap seed、壞設定硬錯。
- fleety-cli:provider/model 命令 parse + apply round-trip;provider_tui 編輯 round-trip。
- `cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-server -p fleety-cli` 無新違規。

**範圍邊界:**
- In scope:providers.toml 兩層資料模型 + validate + migrate + runtime 建構 + pool 聯集能力 + bootstrap seed/硬錯 + provider/model 命令 + provider_tui。
- Out of scope:認證預設開、config local scope 過濾、遠端面板/protocol、傳輸 wss、裝置分級、加密。

## Risks / Trade-offs

- [遷移去重命名衝突] 舊 provider 去重合併時名稱如何選 → 規則明確(首個名或 host 衍生),衝突時保留全部並 warn,不猜。
- [pool 聯集讓客戶端送了某 member 不支援的附件] → 各 member complete() 內建降級,聯集只放寬提示,不會出錯。
- [硬啟動錯誤擋掉原本能跑的部署] → 僅對「存在但壞/不完整」的結構化設定硬錯;完全無設定仍走 env seed / echo,docker 三行 env 不受影響。
- [Strategy 補 single 影響現有 round_robin/failover] → single = 恰一 member 的 pool,沿用 attempt_order,不改現有兩策略語意。
- [provider_tui 大改] → 若互動編輯器改動過大,可先交付非互動命令 + 遷移,TUI 編輯器最小可用即可,缺口列 Notes。
