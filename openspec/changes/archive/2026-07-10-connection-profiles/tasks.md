## 1. fleety-tools 連線模組(連線資料模型與 connections.toml)

- [x] 1.1 在 crates/fleety-tools/src/connection.rs 新增 `Connections { device_id, current: Option<String>, profiles: BTreeMap<String,Profile> }` 與 `Profile { url, token: Option<String>, label: Option<String>, fingerprint: Option<String> }`,以及 `load()`(缺檔回空、壞檔回明確錯誤)、`save()`(tmp+rename 原子、設 0600),交付「Connection profiles are the single persistent source of the connection target」的儲存契約;在同檔單元測試 round-trip(url/token/label 保存)、0600 權限、壞檔回錯而非當空。
- [x] 1.2 在 connection.rs 新增 `migrate_from_config_json()`,實作「config.json migrates once and idempotently to connections.toml」:建 default profile、device_id 以既有值鎖定、url-less 記錄留空 url、舊檔改名 config.json.migrated、connections.toml 已存在則跳過、以 O_EXCL 建檔當閂防併發首啟生兩個 device_id(design「config.json 一次性冪等遷移」)。單元測試:冪等(重跑不再遷移)、device_id 鎖定、url-less 留空、O_EXCL 閂下併發只一個遷移者。

## 2. 共用連線解析器(CLI 與 daemon 共用的連線解析器)

- [x] 2.1 在 connection.rs 新增 `resolve(override) -> (url, Option<token>)`,實作「CLI and daemon share one connection resolver with a single precedence」的單一優先序(-s/--server 或 --url 單次 > env FLEETY_AGENT_URL 臨時 > current profile > mDNS > localhost),env 覆寫永不寫檔、「檔案存在但解析失敗」回錯;並實作「mDNS is a sticky, fingerprint-guarded fallback in the resolver」:enrolled 後不漂移、不把 profile token 送給 fingerprint 不符的 mDNS URL。單元測試覆蓋各優先序分支、env 覆寫不寫檔、sticky、fingerprint guard。

## 3. 移除 registry 的連線鍵(從 config registry 移除 FLEETY_AGENT_URL)

- [x] 3.1 [P] 在 crates/fleety-tools/src/config.rs 的 registry() 移除 FLEETY_AGENT_URL(Daemon scope 那筆),使 `config set FLEETY_AGENT_URL` 回 unknown setting、seed_env_from_config 不再灌它——落實 spec「FLEETY_AGENT_URL is no longer a config key」scenario;調整/新增測試斷言該鍵不在 registry、set 被拒。

## 4. fleety server 命令與 init/pair sugar(fleety server 命令群與 init/pair 收斂為 sugar)

- [x] 4.1 在 crates/fleety-cli/src/main.rs 新增 `fleety server` 子命令 add/use/list/show/current/rename/remove/set-url,交付「The fleety server command group manages named server profiles」:use 只改 current、list 標 current + env 覆寫提示、remove current 需 --force。以 cli_smoke 測試 add→current 印該名、list 標記、remove current 未加 force 被拒。
- [x] 4.2 在 main.rs 把 `fleety init` 收斂為 `server add <name> <url> --use` + enroll、`fleety pair` 對 current profile 配對並把 token 寫回 profile,取代舊 config.json 讀寫(agent_url/saved_token/write_config/fleety_dir),交付「Enrollment operates on connection profiles」。cli_smoke 測試:init 等價 add --use、pair 後 token 落在 current profile、既有 init/pair 呼叫形態不壞。

## 5. daemon 接共用解析器

- [x] 5.1 在 crates/fleety-daemon/src/main.rs 把自有的 env→mDNS→default URL 解析與 fleetyd.token 讀取換成 fleety-tools 的共用 resolver + connections.toml(交付「CLI and daemon share one connection resolver」在 daemon 端),daemon 連 current profile、token 取該 profile;保留 env FLEETY_AGENT_URL 為持久來源。以 fleetyd smoke 測試「舊 env 部署仍可連」與「無 env + 有 connections.toml 連 current」兩條。

## 6. 整體驗證

- [x] 6.1 全 workspace 回歸:`cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-cli -p fleety-daemon` 無新違規;人工確認 config.json 舊裝置遷移後 device_id 不變、connections.toml 為 0600。驗證:三個命令輸出 + 遷移手動檢查。
