## Context

現行連線目標的三處來源與陷阱、`FLEETY_AGENT_URL` 的 scope 錯亂,已在 proposal 與 docs/design-cli-config.md §1 說明。相關程式碼:`fleety-cli` 的 `agent_url()` / `saved_token()` / `write_config()` / `fleety_dir()`(main.rs)讀寫 `~/.fleety/config.json`;`fleety-daemon` 有自己一套 env→mDNS→default 的 URL 解析與 `~/.fleety/fleetyd.token`;`fleety-tools` 的 typed registry(config.rs)含 `FLEETY_AGENT_URL`(Daemon scope)。本 change 把「連哪台 + 認證」抽成獨立的連線層,存 `connections.toml`,由 CLI 與 daemon 共用同一解析器。

## Goals / Non-Goals

**Goals:**
- 連線目標單一持久真相來源(`connections.toml.current`),消除 config.json / config.toml / env 三處打架的優先序陷阱。
- 管理多台具名 server profile 並乾淨切換(`fleety server` 命令)。
- CLI 與 daemon 共用同一解析器與連線儲存(同一台裝置的窗口與手連同一個 server)。
- config.json → connections.toml 一次性、冪等、有備份、併發安全的遷移。

**Non-Goals:**（見 proposal Non-Goals）provider/model、認證預設開、互動面板、遠端 edit protocol、RBAC、加密儲存皆另案。

## Decisions

### 連線資料模型與 connections.toml

新檔 `~/.fleety/connections.toml`(權限 0600):`device_id`(裝置身分,跨 profile 共用)、`current`(當前 profile 名,Option)、`profiles`(name → Profile)。`Profile { url, token: Option, label: Option, fingerprint: Option }`。型別與 load/save 放 `fleety-tools`(新 connection 模組),CLI 與 daemon 共用。load 對缺檔回空(非錯),對存在但解析失敗回明確錯誤(不靜默當空)。save 原子(tmp + rename)並設 0600。

### CLI 與 daemon 共用的連線解析器

新 resolver 取代 `fleety-cli` 的 `agent_url()`/`saved_token()` 與 `fleety-daemon` 各自的解析。單一優先序:(1) 呼叫層傳入的單次覆寫(`-s/--server <name>` 取該 profile;`--url <ws>` 直連,不具名);(2) env `FLEETY_AGENT_URL`(臨時覆寫,永不寫檔;生效時由 `server list`/`status` 頂部提示「env 覆寫中」);(3) `connections.toml.current` 的 `profile.url` + token;(4) mDNS(僅在無 current 時;enrolled 後 sticky,不把某 profile 既有 token 送給 fingerprint 不符的 mDNS URL);(5) `ws://127.0.0.1:8787`。「檔案存在但解析失敗」回錯,不越過 current 去探索。resolver 回傳 (url, token) 供 Hello 使用。

### fleety server 命令群與 init/pair 收斂為 sugar

`fleety server` 子命令:`add <name> <url> [--label][--pair <code>][--use]`、`use <name>`、`list`、`show [<name>]`、`current`、`rename <old> <new>`、`remove <name>`、`set-url <name> <url>`。`list` 標示 current(`*`)、認證狀態、以及 env 覆寫生效時的頂部警告。`remove` current 時要求先 `use` 別台或 `--force`。`fleety init <url> [--name]` 等價於 `server add <name(default)> <url> --use` + enroll;`fleety pair <code>` 對 current profile 配對,minted token 寫回該 profile(取代寫 config.json.token)。舊 `init`/`pair` 呼叫形態保持可用(向後相容)。

### 從 config registry 移除 FLEETY_AGENT_URL

`fleety-tools` 的 `registry()` 移除 `FLEETY_AGENT_URL`(Daemon scope 那筆)。效果:`config set FLEETY_AGENT_URL` 變 unknown key、`seed_env_from_config` 不再灌它進 env——config.toml 這條來源消失。env `FLEETY_AGENT_URL` 仍保留為臨時覆寫(見 resolver)。`FLEETY_DEVICE_ID` 維持在 registry(Daemon scope,fleetyd 身分覆寫用);CLI 的 device_id 改由 connections.toml 提供。

### config.json 一次性冪等遷移

首次讀取時若無 connections.toml 而有 config.json:建 `default` profile（agent_url→url、token→token）、`device_id` 以 config.json 既有值鎖定(勿被 hostname 衍生覆蓋);url-less（僅 token、靠 mDNS）記錄 → profile.url 留空讓 resolver 落 mDNS,不硬填 localhost;寫出 connections.toml 後把 config.json 改名 `config.json.migrated`（備份不刪）。冪等:connections.toml 已存在則不再遷移。併發安全:以 `O_EXCL` 建 connections.toml 當閂,避免 CLI 與同機 daemon 併發首啟各遷移一次、生出兩個 device_id;搶不到閂者等待/重讀既有檔。

## Implementation Contract

**行為:**
- `fleety server add home ws://… --use` 後 `fleety server current` 印 `home`,`fleety tui`/`ask` 連到該 url。`server use <name>` 只改 current 一個欄位,CLI 與本機 daemon 下次連線都用它。
- `fleety init ws://x` 與舊版行為等價(建/更新 default profile 並切過去),既有腳本不壞。
- `config set FLEETY_AGENT_URL …` 回 unknown setting（已從 registry 移除）。
- 有 config.json 無 connections.toml 的舊裝置,任一 fleety/fleetyd 首次啟動後 connections.toml 生成、config.json.migrated 出現、device_id 不變。
- daemon 與 CLI 用同一 connections.toml.current;`server use` 後 daemon 下次連線改連新 server（token 取該 profile 的）。

**介面 / 資料形狀:**
- `~/.fleety/connections.toml`：`device_id`、`current`、`[profiles.<name>] url/token/label/fingerprint`。
- `fleety-tools` 連線模組公開：`Connections`、`Profile`、`load()/save()`、`resolve(override) -> (url, Option<token>)`、`migrate_from_config_json()`。

**失敗模式:**
- connections.toml 存在但壞 → 明確錯誤（不靜默當空、不越過 current）。
- `server remove` 當前 profile 未加 `--force` → 拒絕並提示先 `use` 別台。
- env 覆寫生效 → 不寫檔、`server list`/`status` 頂部提示。
- 併發首啟 → O_EXCL 閂確保單一遷移者,不生重複 device_id。

**驗收條件:**
- fleety-tools 單元測試:Connections load/save round-trip、0600、resolve 各優先序分支、migrate 冪等 + device_id 鎖定 + url-less 留空。
- fleety-cli 測試（含 cli_smoke）:`server add/use/list/current`、`init` 等價於 add --use、`pair` 寫回 profile token、`config set FLEETY_AGENT_URL` 回 unknown。
- fleety-daemon 測試:daemon 用共用 resolver 連 current profile；舊 env FLEETY_AGENT_URL 部署仍可連。
- `cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens。

**範圍邊界:**
- In scope:連線層資料模型 + 命令 + 共用 resolver + registry 移除 FLEETY_AGENT_URL + config.json 遷移。
- Out of scope:provider/model、認證預設開、互動面板、遠端 edit protocol、加密儲存、多裝置身分/RBAC。

## Risks / Trade-offs

- [daemon resolver 改動連不上] daemon 現況 env→mDNS→default,改共用 resolver 後若遷移或 current 未設會落 mDNS/localhost。→ 保留 env `FLEETY_AGENT_URL` 為持久來源(對 unit 檔),並補「舊 env 部署」與「無 env + 有 connections.toml」兩條 daemon smoke。
- [CLI 與 daemon 併發首啟撞遷移] → O_EXCL 閂 + 搶不到者重讀。
- [connections.toml 集中多 token 放大爆炸半徑] → 本 change 只保證 0600 + fingerprint 欄位;keychain/加密留後續強化(Non-Goal)。
- [mDNS 漂移到 rogue server] → enrolled 後 sticky,不把 profile token 送 fingerprint 不符的 mDNS URL。
