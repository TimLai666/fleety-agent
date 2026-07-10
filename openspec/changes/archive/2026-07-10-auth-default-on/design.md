## Context

現況(讀程式碼驗證):`fleety-server` 的 `main.rs` 以 `std::env::var("FLEETY_REQUIRE_AUTH").as_deref() == Ok("1")` 讀取(預設 false),再建 `AuthStore::load(path, FLEETY_TOKEN, require_auth)`;`main.rs` 開機已跑 `seed_env_from_config`(config.toml 值會進 env)。`AuthStore`(auth.rs):`required`、`bootstrap`(FLEETY_TOKEN admin token)、`verify`(bootstrap 或 device token)、`create_pairing`/`redeem`。連線在 Hello 時檢查(conn.rs `if !auth.required() {...}`),故 auth 關閉時任何連線放行。`ConfigExec` handler(conn.rs)註解明說「auth 關閉時遠端 config 無實質閘」,直接 `config_apply(target,args)`;`config_effect(args)` 對 mutating verb 回 `Some(NextConnection|Restart)`、對讀回 `None`。registry(fleety-tools config.rs)`FLEETY_REQUIRE_AUTH` 預設 `"0"`、validator `v_bool`。

## Goals / Non-Goals

**Goals:** 認證預設開(未顯式關即開);首啟無入口時引導配對;auth 關閉的 server 拒收 mutating 遠端 config frame(可讀不可寫)。

**Non-Goals:**（見 proposal）敏感 key 告警/稽核、wss/TLS 要求、配對強化、snapshot 分級、遠端 edit protocol/面板、裝置分級、FLEETY_ADDR 預設。

## Decisions

### FLEETY_REQUIRE_AUTH 預設翻為開

registry(fleety-tools config.rs)`FLEETY_REQUIRE_AUTH` 預設 `"0"`→`"1"`,描述改為「預設要求 token 連線(1/0);設 0 才關」。`main.rs` 的讀取由 `== Ok("1")` 改為 `!= Ok("0")`(未設即開、顯式 `0` 才關)。因 `seed_env_from_config` 先跑,config.toml 的值會進 env,故三層(env>config>default)一致落在「未顯式關即開」。既有把它設為 `0`/`1` 的部署不受影響,只有「完全沒設」的預設從關翻成開。

### 首啟配對引導(避免 auth-required 變無法配對的磚)

`AuthStore` 新增 `is_uninitialized()`:`bootstrap.is_none()` 且無任何 device token(第一台都還沒配)。`main.rs` 建完 `AuthStore` 後,若 `require_auth && auth.is_uninitialized()`,呼叫 `create_pairing()` 產一次性配對碼並以顯眼 log 印出下一步(`fleety init <server-url>` 後 `fleety pair <code>`,10 分鐘內有效)。有 bootstrap token 或已有配對裝置則不印(非首啟)。

### 遠端寫入 ⇒ 認證必開(auth 關閉時拒收 mutating frame)

`ConfigExec` handler(conn.rs)在呼叫 `config_apply` 前先判斷:`config_effect(&args).is_some()`(即這是 mutating verb)且 `!auth.required()` → 直接回 `ConfigResult { ok:false, error: unauthenticated }`,訊息指引先開認證(`FLEETY_REQUIRE_AUTH=1` 後重啟);讀取(effect None)照常。auth 開啟時所有 frame 照舊(連線已在 Hello 驗證)。`config_apply` 簽章不動,閘加在 handler。

## Implementation Contract

**行為:**
- 未設 `FLEETY_REQUIRE_AUTH` 開機 → server 要求 token 連線(預設開);顯式 `FLEETY_REQUIRE_AUTH=0` → 不要求(照舊)。
- auth-required 且無 bootstrap token、無已配裝置的首啟 → log 一組配對碼與 `fleety pair <code>` 指引。
- auth 關閉的 server 收到 `config set …` / `provider add …` 等 mutating frame → 回 unauthenticated 錯誤、不改設定;收到 `config list`/`get` 讀取 → 照常回。
- auth 開啟的 server → mutating 與讀取都照舊(連線已驗證)。

**介面 / 資料形狀:**
- registry `FLEETY_REQUIRE_AUTH` default `"1"`。
- `main.rs`:`require_auth = env != Ok("0")`。
- `AuthStore::is_uninitialized() -> bool`。
- `ConfigExec` handler 前置閘用既有 `fleety_tools::config::config_effect`。

**失敗模式:**
- auth 關閉 + 遠端 mutating config → `ConfigResult { ok:false, error.kind="unauthenticated" }`,不落檔。
- 首啟 `create_pairing` 失敗 → warn（不阻擋開機）。

**驗收條件:**
- fleety-tools 單元:`FLEETY_REQUIRE_AUTH` 預設解析為 `"1"`。
- fleety-server 單元:`main.rs` require_auth 預設讀取為 true、顯式 `0` 為 false;`AuthStore::is_uninitialized` true/false 各情境;`ConfigExec` 閘(auth 關閉時 mutating 拒、讀放行;auth 開啟時 mutating 放行)。
- `cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-server` 無新違規。

**範圍邊界:**
- In scope:REQUIRE_AUTH 預設翻開 + 首啟配對引導 + 遠端寫入認證閘。
- Out of scope:敏感 key 告警/稽核、wss、配對強化、snapshot 分級、遠端 edit protocol/面板、裝置分級、FLEETY_ADDR。

## Risks / Trade-offs

- [預設翻開破壞「開箱即連」] 既有無 auth 部署預設要配對 → 這正是安全修補(design 硬底線);首啟引導 + 顯式 `FLEETY_REQUIRE_AUTH=0` 逃生門降低衝擊;既有顯式設值不受影響。
- [server_smoke 等測試] `invalid_bind_exits_after_startup_setup` 不建連線,不受影響;若有靠預設關連線的測試需補設 `FLEETY_REQUIRE_AUTH=0` 或給 token。
- [config_effect 判斷 mutating 不精準] 以既有 `config_effect`(provider/model/set/unset → Some)為準,與 audit 判定同源,一致。
- [首啟配對碼印在 log] 短效(10 分鐘)+ 僅首啟印;風險可接受,強化(單一活躍碼/節流)留 §4 額外防線另案。
