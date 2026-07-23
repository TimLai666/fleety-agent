# Fleety CLI 設定架構重新設計(定稿)

_狀態:設計定稿並已實作。共享 terminal workspace 的 Settings 分為 Connection / CLI / Daemon / Server / Providers & Models 五頁。CLI 只透過本機 CLI owner service 套用自身設定；Daemon、Server、Provider、Model、OAuth 一律送到擁有它的 runtime，不提供直接改檔 fallback。2026-07-17。_

本檔是 CLI 設定架構的重新設計。經一輪多方案設計 + 一輪六角度紅隊(7 blocker /
20 high)+ 與擁有者逐項拍板收斂而成。實作分兩階段(見 §10)。

---

## 1. 動機:現況把「連線」和「設定」混談,各有兩套存法打架

三個確認的病根(讀程式碼驗證):

1. **連線目標三處存、有優先序陷阱**:`~/.fleety/config.json` 的 `agent_url`
   (`fleety init` 寫)/ `config.toml` 的 `FLEETY_AGENT_URL`(registry `Daemon`
   scope,`config set` 寫、開機 seed 進 env)/ env `FLEETY_AGENT_URL`。解析優先序
   讓 `config set FLEETY_AGENT_URL` **靜默蓋過** `fleety init`,使用者看不出為何
   init 沒生效。
2. **模型設定概念錯位**:扁平 `FLEETY_MODEL_BASE_URL` / `FLEETY_MODEL` /
   `FLEETY_MODEL_KEY` 把 provider 的屬性(端點、金鑰)攤在 model 名下——`key` /
   `base_url` 其實屬於 provider,不屬於 model。`providers.toml` 方向對一半,但
   把 `model` 綁在 provider 裡(一個 provider 一個 model),做不到「同一個
   provider 給主模型用 A、便宜模型用 B」。
3. **遠端設定只能非互動**:`ConfigExec` 送 args 字串、回 rendered 純文字,結構上
   無法做遠端互動 edit(要拉 server 的結構化 config 到本機 ratatui 顯示)。

`FLEETY_AGENT_URL` scope 還被標成 `Daemon`,而 `Cli` scope 全域只有 2 個 voice
設定——CLI 這個永遠跑在裝置端的東西,連自己最核心的「連哪台」都沒有正當歸屬。

---

## 2. 核心:三層徹底分離

| 層 | 是什麼 | 單一真相來源 | 主要入口 |
|---|---|---|---|
| **Connection** | 連哪台 server + 認證(裝置端特有,本質不是設定旋鈕) | `connections.toml`(CLI+daemon 共用) | `fleety connection …` / Settings (1) |
| **CLI** | 這台裝置的 `fleety` 自己的行為(voice、顯示) | `config.toml` 的 `Cli` | `fleety config … --owner cli` / Settings (2) |
| **Daemon** | 這台裝置背景 `fleetyd` 的行為與 Shared 設定 | daemon 的 `config.toml` | `fleety config … --owner daemon` / Settings (3)，經 daemon tool bridge |
| **Server** | 遠端那台 server 的一般設定(policy、addr…) | server 那端的 registry | `fleety config … --owner server` / Settings (4) |
| **Providers & Models** | Provider、OAuth、模型 catalog 與 role | 目前連線 Server 的 provider service | `fleety provider …` / `fleety model …` / Settings (5) |

「連哪台」不是 registry setting(它是指標/目標,還要能管多台切換);「這台 CLI 怎麼
跑」和「server 怎麼跑」是兩台機器的兩份設定。三者各有唯一權威來源,結構上不重疊。

---

## 3. 資料模型

### 3.1 連線層 — `~/.fleety/connections.toml`(新)

CLI 與 daemon **共用**(同一台裝置的窗口與手連同一個 server;見 §決策 M6)。

```toml
device_id = "…"          # 裝置身分,跨 profile 共用(唯一權威,見 M7)
current = "home-pi"      # 當前連哪台(CLI 與 daemon 都用它)

[profiles.home-pi]
url   = "ws://192.168.1.10:8787"
token = "…"              # 該 server 對本裝置發的 token(per-server keyed,見 M6)
label = "客廳那台"        # 選填
fingerprint = "…"        # server 憑證/身分指紋,做 pinning 防漂移(見 M6 安全)

[profiles.office]
url   = "wss://office.example:8787"
token = "…"
```

- 檔案權限 **0600**(集中多把 token,見 §9 安全)。
- 型別在 `fleety-tools` 共用模組(CLI + daemon 同一 resolver,見 M6):
  `Connections { device_id, current: Option<String>, profiles: BTreeMap<String, Profile> }`;
  `Profile { url, token: Option<String>, label: Option<String>, fingerprint: Option<String> }`。
- 取代現行 `config.json` 的 `agent_url`/`token`/`device_id` 三個欄位與
  `write_config`/`saved_token`/`agent_url` 的讀寫。

### 3.2 本機 CLI 設定 — `config.toml` 的 `Cli`/`Shared`

維持現行 typed registry,但:
- **移除 `FLEETY_AGENT_URL`**(連線層接手,消除優先序陷阱)。
- Settings CLI 頁與 `config … --owner cli` 只顯示 `Cli` scope。Shared 固定由 daemon 擁有。
- 每個 `Shared` 鍵定**單一權威來源**(見 M7),colocation(server+daemon 同機)時
  不再兩處各改一份。

### 3.3 Server 的 Provider / Model 兩層(取代扁平 `FLEETY_MODEL_*`)

存在 **server 那端**(server 才呼叫模型)。

```toml
# ── Provider:具名,type 可擴展的 tagged enum ──
[providers.openai1]
type     = "api"                       # api = base_url + key
base_url = "https://api.openai.com/v1"
key      = "sk-…"                       # secret

[providers.codex1]
type = "oauth:codex"                   # oauth 型:token 由 fleety provider login codex1 產,per-provider

[providers.google1]
type     = "api"
base_url = "https://generativelanguage.googleapis.com/…"
key      = "…"

# ── Model role:固定 main / cheap;每個是一個 pool ──
[models.main]
strategy = "failover"                  # single | round_robin | failover
members = [
  # member = 完整建構單元:model 級屬性(stream/modalities/effort)下沉到這裡(見 M2)
  { provider = "openai1", model = "gpt-4o",           stream = true, modalities = "text,image", effort = "medium" },
  { provider = "google1", model = "gemini-2.5-pro",   stream = true, modalities = "text,image" },
]
[models.cheap]
members = [
  { provider = "openai1", model = "gpt-4o-mini" },    # 同一個 provider,不同 model
]
```

**關鍵規則(補 M2):**
- **`key` / `base_url` / oauth token 屬 provider**,不屬 model。`stream` / `modalities`
  / `effort` 是「跟著模型/該次呼叫」的屬性,**下沉到 member**(不是 provider、不是
  role);現行 `ProviderSpec` 的這三欄遷移時逐筆搬到對應 member。
- `provider` 依 `type` 做 **tagged enum**:`api` 必填 `base_url`,禁 `token`;
  `oauth:*` 由登入產 token,禁 `base_url`/`key`。`type` 做成**可擴展註冊**(未來加別種
  oauth 型,不改核心 if)。
- **混族 pool**(一個 role 放不同模型)**允許**:能力**依這次實際選到的 member 動態
  決定**——附件走原生還是降級、送不送 effort,延到路由選定 member 之後才判斷,不在
  pool 層預先取 first 或交集(這是對現行 `PoolProvider::capabilities()` 取
  `members.first()` 同質假設的正面推翻,見 M2)。
- **參照完整性**(補 M2/should-fix):`members[].provider` 必須是已定義 provider;
  刪 provider 前先擋(被引用則拒或 `--cascade`);`strategy=single` 需 `members`
  恰一個;寫入前跑 validate,**不沿用 runtime 的 fail-soft 靜默丟棄**。

---

## 4. 認證與授權(安全)— 補 M1

擁有者定案:**手機/筆電上要能遠端改 server 的所有設定,含「門鎖」key**
(`FLEETY_POLICY` / `FLEETY_REQUIRE_AUTH` / `FLEETY_TOKEN`)。要安全地做到:

**硬底線(不可妥協):認證改成預設要。**
- 現況 `FLEETY_REQUIRE_AUTH` 預設 `0`——**任何人連得到就能用,不需配對**。這是紅隊
  的洞,也不符「連線本來就該驗證」的直覺。
- 改為:**首次啟動 server 引導設定認證(等同 `REQUIRE_AUTH` 預設開)**,連線本來就要
  配對。能連上的都是**配對過、信任的裝置**。
- **遠端寫入 ⇒ 認證必開**:若某 server 在 `REQUIRE_AUTH=0` 狀態,一律**拒收任何
  mutating config frame**(可讀不可寫),或第一次遠端改設定時先引導開認證再放行。

**在「認證預設開」之上,配對過即信任裝置 → 遠端改任何東西(含門鎖)放行**,不強制
「主人/一般」分級(單使用者 fleet,配對過的就那幾台)。但保留分級能力為進階選項
(防裝置遺失/借用),預設不啟用。

**額外防線(補 M1 其餘 blocker/high):**
- **敏感 key 覆寫告警 + 稽核**:改到「會導致外流」的 key(provider `base_url`/`key`、
  `FLEETY_BACKUP_REPO`/`_TOKEN`、oauth endpoint URL),套用前醒目告警並記稽核(新舊
  值 host),避免有人偷把對話/備份導向他處。`base_url` 不再當無害旋鈕。
- **傳輸**:承載 secret / mutation 的連線在**非 loopback 強制 `wss`**(或非 TLS 直接
  拒收 secret 寫入與配對);`connections.toml` 記 server `fingerprint` 做 pinning。
- **配對強化**:加長配對碼、限單一活躍碼、`redeem` 加來源節流 + 失敗鎖定(現行 32-bit
  / 10 分鐘 / 無節流可暴力破解)。
- **snapshot 讀取分級**:`ConfigSnapshot` 對敏感欄位只回 `is_set`、記錄誰讀過(避免
  低信任裝置無稽核偵察安全態勢)。

---

## 5. 連線解析(單一路徑,消除 M1-P1 陷阱)

持久來源只剩 `connections.toml.current`。`agent_url` 新解析(CLI 與 daemon 共用同一
resolver):

1. **單次覆寫**:`--profile <name>`(選一個既有 profile 跑這一次)或
   `-s/--server <ws-url>`(不具名單次直連)——本次呼叫、**不寫檔、不動 daemon**。
   (這是「CLI 臨時連別台看一眼」的正道,見 M6。)
2. **env `FLEETY_AGENT_URL`**:保留為**唯一的臨時 env 覆寫**——永不寫檔、永不從
   `config.toml` seed;生效時 `server list`/`status` 頂部**醒目提示**「env 覆寫中,
   略過 profile <current>」。對背景 daemon,env 是 unit 檔裡的**持久**設定(見 M6)。
3. `connections.toml.current` 的 `profile.url`。
4. mDNS(短探測)——TXT `fingerprint` 只是不可信提示;automatic discovery
   **一律不附 stored token**,credentialed endpoint 改變需明確重選並重新配對。
5. `ws://127.0.0.1:8787`。

「檔案存在但解析失敗」要**報錯**,不可靜默越過 current 去探索(補 M6)。

---

## 6. 命令面(非互動,給腳本/自動化)

**連線管理(改連哪台 / 多台切換):**
```sh
fleety connection add <name> <url> [--label …] [--use]
fleety connection use <name>          # 切換 current(CLI+daemon 一起,見 M6)
fleety connection list | show [<name>]
fleety connection rename <old> <new> | remove <name> | set-url <name> <url>
fleety init <url> [--name <name>] # guided add + use
fleety pair <code>                # 對 current 配對；--profile <name> 精確配對其他 profile
fleety --profile <name> <cmd> | --server <ws> <cmd>   # 單次覆寫,不改 current、不動 daemon
```

**本機 CLI 設定:**
```sh
fleety config --owner cli list | get <KEY> | set <KEY> <VALUE> | edit   # 只 Cli
fleety config --owner daemon list | get <KEY> | set <KEY> <VALUE>       # Daemon/Shared，送至 fleetyd
```

**Server 設定(遠端,對 current profile):**
```sh
fleety config set <KEY> <VALUE> | get <KEY>                       # 依 key owner 自動路由
fleety provider add <name> --type api --base-url … --key …
fleety provider add <name> --type oauth:codex             # 再 fleety provider login <name>
fleety provider edit|remove|list
fleety model catalog <provider> --role main
fleety model set main  --member openai1/gpt-4o --member google1/gemini-2.5-pro --strategy failover
fleety model set cheap --member openai1/gpt-4o-mini
```

**認證(per-provider,可擴展):**
```sh
fleety provider login <provider> | status <provider> | logout <provider>   # codex1/codex2 各自,token per-provider
```

**daemon 連線(用 CLI 設,見 M6):**
```sh
fleety connection use <name>          # 同時設定 CLI 與本機 daemon(一台一個連線)
# daemon 無互動,連線由 CLI 命令 / 面板設;env 對 daemon 是持久來源
```

**相容 alias 與輸出契約:**

| 舊拼法 | canonical |
|---|---|
| `fleety tui` | `fleety chat` |
| `fleety server …` | `fleety connection …` |
| `fleety auth login\|logout\|status …` | `fleety provider login\|logout\|status …` |
| `fleety config provider\|model …` | `fleety provider\|model …` |
| `fleety config --target …` | `fleety config --owner …` |

這些 alias 在任何 I/O 前正規化成相同 typed command。`--json` 固定輸出
`schema_version / ok / context / data / errors`。成功 exit 0，runtime/owner 失敗
exit 1，usage 失敗 exit 2。多 owner read 即使部分失敗也保留已取得資料，human
標示 `PARTIAL`，JSON 設 `ok:false` 並列出每個 owner error，process exit 1。
Mutation 永遠只解析到一個 owner，失敗不直接改 owner 的設定檔。

---

## 7. 共享 terminal workspace 的 Settings

裸 `fleety config` 在 TTY 直接開共享 workspace 的 Settings。Tab 在五個 owner-aware
頁面移動,**不用打 `--owner`**。非 TTY 使用 generated help / 明確子命令。

> Provider/Model/OAuth 編輯的是**目前連線的 Server**，走 snapshot → stage →
> `ConfigApply`，不是本機檔。新增 `oauth:codex` 可直接登入，既有 OAuth provider
> 可登入、登出、換帳號。所有頁面共用 profile/identity/model header、notice 與 footer。
> profile 切換若舊 profile 有 dirty remote state，必須先 Apply / Discard / Cancel；
> 選擇成功持久化後才關閉舊 transport，並重新載入新 Server/Daemon snapshot。

```
┌─ fleety config ─────────────────────────────────────┐
│ [1] Connection [2] CLI [3] Daemon [4] Server [5] Providers & Models │
├──────────────────────────────────────────────────────┤
│ (1) Connection                                       │
│   這台 CLI 連 →  home-pi  ws://192.168.1.10:8787 ● 連線│  Enter=切換
│   背景 daemon 連 → home-pi                            │  Enter=改(見 M6)
│     office        wss://office.example:8787           │
│   + 新增 (a)   配對 (p)   刪除 (d)                     │
│                                                      │
│ (2) CLI — 這個 CLI process 自己的旋鈕                │  只 Cli
│ (3) Daemon — 這台裝置的背景 runtime                  │  Daemon/Shared，經 fleetyd
│     語音模式         auto                              │
│     傳輸(WS/SSE)    auto                              │
│     顯示時區         Asia/Taipei                       │
│                                                      │
│ (4) Server — home-pi 上的一般設定                     │  拉自連線 server
│   一般:policy=full_access  addr=…  tz=…              │  門鎖 key 標示需認證
│ (5) Providers & Models — home-pi                     │
│   Providers:                                         │
│     openai1  api    api.openai.com   key=****         │  a 新增 e 編輯 d 刪
│     codex1   codex  <帳號A>  ● 已登入                  │
│   Models:                                            │
│     main   failover  [openai1/gpt-4o, google1/gemini] │  選 provider+model
│     cheap  single    [openai1/gpt-4o-mini]            │
└──────────────────────────────────────────────────────┘
```

- **(4) Server 與 (5) Providers & Models 的編輯都經連線遠端套用**。編輯 provider(`e`)
  依 `type` 顯示不同欄位:api 型預填 base_url+key(空 key 保留原值);`oauth:codex` 型
  不填欄位,改進**登入/登出/換帳號子選單**——因為登入是 async+開瀏覽器+搶終端,
  做法是存檔→離開全螢幕→對該 provider 跑 `auth login/logout`(換帳號=登出再登入)→
  重開編輯器。憑證 per-provider(每個 codex provider 各自帳號)。OAuth row 同時查詢
  server-side credential status，明確標示「已登入／未登入／無法取得狀態」，不把未知誤報
  成未登入。設定 model 時，API provider 查自己的 `/models`，`oauth:codex` 由 server
  用該 provider 的 OAuth credential 查 Codex catalog，查不到就保留手動輸入。門鎖 key
  標示「需認證已開」;敏感 key 覆寫前二次確認 + 告警。
- secret 顯示遮罩,編輯 secret 只上送新值(write-only,tri-state:keep/set/clear,
  遮罩值一律不回寫,見 M3)。
- 生效時機(下次連線 / 需重啟)在 detail pane 明示。

---

## 8. Protocol:遠端互動 edit(Phase 2)— 補 M3/M4

現行 `ConfigExec` 送字串回文字,做不到互動 edit。新增結構化通道:

```
ClientMsg::ConfigSnapshot { target }
  → ServerMsg::ConfigSnapshotResult {
       revision,                    // 檔案 hash + server boot id(樂觀鎖,補 M3)
       entries: [ ConfigEntry { key, scope, value, default, description,
                                secret, is_set, effect, choices } ],
       // provider/model 以結構化子區帶回
     }
ClientMsg::ConfigApply { target, base_revision, changes: [ ConfigChange { key, op, value } ] }
  → ServerMsg::ConfigResult { ok, output, effect, error }
     // base_revision 不符 → 回 conflict(補 M3 lost-update);sparse(只送觸碰的鍵)
     // change.op ∈ { keep, set, clear }(secret tri-state)
```

- **真原子**(補 M3):server 端 `config.toml` save 改 **tmp + rename + 單一
  per-file mutex**(比照 providers);壞檔**不 fail-soft 退 default**,回明確錯誤。
- **前向相容**(補 M4):`Welcome` 加 additive 的**能力/協定版本欄位**(舊 server 送
  `None`);CLI 據此走 Snapshot 或退回舊 `ConfigExec`。server 收到未知 frame 從
  「serde 失敗硬斷連線」改為「**回 unsupported 錯誤 frame、連線續存**」——否則未來任何
  additive frame 都會炸連線。`PROTOCOL_VERSION` +1。
- **授權**(接 §4):`ConfigApply` 對 Server scope 的 mutate 要求認證已開;門鎖 key
  走 §4 規則;`ConfigSnapshot` 敏感欄位分級 + 稽核。

---

## 9. 遷移(向後相容,一次性、冪等、有備份)

- `config.json`(agent_url/token/device_id)→ `connections.toml`:建 `default`
  profile;`device_id` **以 config.json 既有值為準鎖定**(勿被 hostname 衍生覆蓋,
  補 M7);url-less記錄 → `profile.url` 留空且保留 token 供明確復原,但 resolver
  不得把 token 交給 mDNS 結果或硬填 localhost。`config.json` 改名
  `.migrated` 保留備份,不刪。
- `providers.toml`(provider 綁 model)→ 新兩層:「同 `base_url`+`key` 只差 model」
  的舊 provider **去重合併**成單 provider 多 member(否則產出一堆重複 provider,正是
  要消滅的反模式);`stream`/`modalities`/`effort` 逐筆搬到對應 member,**搬不動要明確
  報警而非默默吞掉**(補 M2)。
- 扁平 `FLEETY_MODEL_*` env → **保留為 bootstrap seed(M5)**:未設結構化 provider/model
  時,自動組成 `models.main`;`docker-compose` / CI 三行 env 照樣可跑。壞/缺結構化設定
  改**硬啟動錯誤**(拒開機),不再靜默退 echo;保留 echo 佔位僅用於「連線可驗證」。
- **遷移併發保護**:CLI 與同機 daemon 首啟可能同時遷移 → 單一遷移擁有者 + 檔鎖
  (`O_EXCL` 建檔當閂),避免生出兩個 `device_id`(補 M3/M7)。遷移前備份、遷移後清除
  舊檔殘留 secret;`config.toml` save 補 0600。

---

## 10. 分兩階段(降風險,先出價值)— 兩階段皆已出貨(2026-07-10)

**Phase 1(不動 wire,全 additive、可降級):**
- `fleety-tools` 共用連線模組 + `connections.toml` + `fleety connection …` 命令
  + CLI/daemon 共用 resolver + 遷移 + `FLEETY_AGENT_URL` 移出 registry。
- Provider/Model 兩層資料模型(member 屬性下沉、混族動態能力、參照完整性 validate)
  + `providers.toml` 遷移 + `FLEETY_MODEL_*` 降為 bootstrap seed。
- 認證預設開(首次啟動引導)+ 「遠端寫入⇒認證必開」硬前置。
- `fleety config open`（legacy `config edit` alias）統一進共享 Settings，依 owner
  暫存後才 Apply，不再保留另一套直接寫檔 editor。
- **交付**:G1(乾淨改連哪台、多台切換、無陷阱)+ 乾淨的 provider/model + 安全底線。

**Phase 2(動 wire,交付互動全包):**
- `ConfigSnapshot`/`ConfigApply` + revision 樂觀鎖 + 真原子 + 能力協商 + secret
  tri-state。
- 共享 workspace Settings(五頁)+ 遠端互動 edit + 敏感 key 授權/告警/稽核 + 傳輸 wss 要求。
- **交付**:G2(一個面板設定任何東西,含 server 全設定)。

---

## 11. 決策記錄

| 決策 | 結論 |
|---|---|
| **M1 安全閥遠端可改?** | **可以**——但配「認證改成預設要」+「遠端寫入⇒認證必開」;配對過即信任裝置故不強制 owner 分級(留為進階);敏感 key 告警+稽核+wss。 |
| **M2 混族 pool?** | **允許**,能力依這次實際選到的 member **動態**決定;member 為完整建構單元(stream/modalities/effort 下沉)。 |
| **M5 廢扁平 env?** | **不廢**,降為 bootstrap seed(headless/CI/Docker 逃生門);壞設定硬錯不退 echo。 |
| **M6 daemon 連線?** | **CLI 與 daemon 共用一份連線(一台一個 server,窗口+手連同一大腦)**;切換整台一起;臨時看別台用 `-s`/`--url` 單次旗標;由 CLI 命令/面板設定。 |
| model role | 固定 **main + cheap**。 |
| provider type | **可擴展註冊**(不寫死只有 codex)。 |

紅隊 7 blocker 對應:M1→§4,M2→§3.3,M3→§8,M4→§8,M5→§9,M6→§5/§6,M7→§3.2/§9。

---

## 12. 後續/未決

- 「主人/一般」裝置分級的具體機制(若日後啟用):owner 標記如何持久化、如何在既有
  owner 裝置上升級一台。
- 遠端 provider TUI(providers 池的完整互動編輯)——Phase 2 面板已含,細節按實作收斂。
- `FLEETY_ADDR` 預設是否配合改 `0.0.0.0`(與認證預設開一起評估;Docker 映像已預設)。
