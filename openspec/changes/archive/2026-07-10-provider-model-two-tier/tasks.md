## 1. 兩層資料模型(Provider 的 type-tagged enum 資料模型 / Model role 兩層 pool 與 member 屬性下沉)

- [x] 1.1 在 crates/fleety-tools/src/providers_config.rs 以 `type`-tagged enum 重構資料模型:`Provider`(api:base_url+可選 key;oauth:codex:無 base_url/key)、`Member { provider, model, stream, modalities, effort }`、`ModelPool { strategy, members }`、`ProvidersConfig { providers: BTreeMap<String,Provider>, models: BTreeMap<String,ModelPool> }`;`Strategy` 補 `single`;`parse`/`load_from`/`load_or_default` 解析新格式、未知 `type` 回列出已知 type 的錯誤,交付「Providers are a type-tagged enum separating endpoint from model」與「Model roles are member pools with call-time traits on the member」。單元:tagged provider + member pool round-trip、未知 type 報錯列已知、api/oauth 欄位形狀。
- [x] 1.2 在 providers_config.rs 實作 `validate(cfg)` 與 `write_providers` 寫前驗證(design「參照完整性 validate(寫入前,非 runtime fail-soft)」),交付「Referential integrity is validated before write, not fail-soft」:member.provider 必須已定義、`strategy="single"` 恰一 member、api 缺 base_url 或帶 token 拒、oauth 帶 base_url/key 拒、刪被 role 引用的 provider 拒並指出 role;壞設定不落檔(runtime load_from 仍 fail-soft)。單元覆蓋各拒項 + 一致設定通過。
- [x] 1.3 在 providers_config.rs 新增 `migrate_providers`(design「providers.toml 一次性去重遷移」),交付「providers.toml migrates once to the two-tier shape」:同 base_url+key+auth 只差 model 的舊 provider 去重合併成單 provider 多 member、stream/modalities/effort 逐筆下沉到對應 member、舊 roles/groups 對應 models.<role> members、搬不動欄位明確 warn 不吞、已是新格式(有 [models.*])則冪等跳過。單元:去重+屬性下沉、idempotent、warn 列出搬不動項。

## 2. Runtime 建構與能力(混族 pool 的動態能力 / FLEETY_MODEL_* bootstrap seed 與壞設定硬啟動錯誤)

- [x] 2.1 [P] 在 crates/fleety-server/src/pool.rs 把 `PoolProvider::capabilities()` 從 `members.first()` 改為跨 members 聯集(任一支援→支援)(design「混族 pool 的動態能力(pool capabilities = member union)」),交付「A pooled provider reports homogeneous capabilities」(改為聯集)與「Providers report their modality capabilities」的 pool 聯集條款;更新 `capabilities_come_from_first_member` 測試為聯集斷言(text-only + image-capable → image 支援)。
- [x] 2.2 在 crates/fleety-server/src/providers.rs 讓 `ProviderTiers::from_config` 依新兩層模型建構:先建 provider 連線參數,再對每個 role 用其 members 經 `build_provider`(provider 的 base_url/key/auth + member 的 model/stream/modalities/effort)建一個 `PoolProvider`,`resolve("main"|"cheap")` 回該 pool、未知 selector 回 main,交付「Model roles are member pools with call-time traits on the member」runtime 端。單元:from_config 建 main/cheap member pool、resolve、一 provider 兩 role 兩 model。
- [x] 2.3 在 providers.rs / server 啟動路徑實作 bootstrap seed 與硬錯,交付「Flat model env is a bootstrap seed and broken structured config is a hard error」:無結構化設定時 FLEETY_MODEL_*/FLEETY_CHEAP_MODEL_* 組 models.main(+cheap);providers.toml 存在但壞/參照不完整→server 硬啟動錯誤回明確訊息(不退 echo);完全無設定保留 echo 佔位。單元:三行 env 組出 main、壞結構化設定回錯(非 echo)。

## 3. 命令與互動編輯器(provider/model 命令與互動編輯器改新模型)

- [x] 3.1 在 crates/fleety-tools/src/config.rs 把 `ProviderCmd`/`parse_providers`/`run_providers_at` 改為新模型的 provider 命令:`provider add <name> --type api --base-url … [--key …]` / `--type oauth:codex`、`provider set/remove/list`(list 依 type 顯欄位、mask key、remove 被引用時拒),交付「config subcommands manage providers, groups, and roles」的 provider 半。單元:provider add(兩 type)parse + apply round-trip、oauth 禁 base_url/key、remove 被引用拒。
- [x] 3.2 在 config.rs 新增 model role 命令:`config model set <main|cheap> --member <provider>/<model> [--stream][--modalities …][--effort …] [--member …] --strategy <single|round_robin|failover>`、`model show`、`model unset`,交付「config subcommands manage providers, groups, and roles」的 model 半與「Model roles are member pools…」命令面。單元:model set 多 member + strategy parse + apply、single 非恰一 member 拒、show/unset round-trip。
- [x] 3.3 在 crates/fleety-cli/src/provider_tui.rs 把互動編輯器改對應新兩層結構:provider 依 type 顯示不同欄位(api 要 base_url+key;oauth 顯登入狀態)、model role 編輯 member 清單(選 provider+model + 三屬性 + strategy),交付新模型的「An interactive screen manages providers on a TTY」延續。單元:TUI state 對新模型的編輯 round-trip(至少 provider 新增 + model member 設定各一)。

## 4. 遷移驗證與回歸

- [x] 4.1 全 workspace 回歸:`cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-server -p fleety-cli` 無新違規;人工以舊格式 providers.toml 跑一次確認去重遷移正確、混族 pool audio_input 為聯集、壞設定硬錯。驗證:命令輸出 + 遷移手動檢查。
