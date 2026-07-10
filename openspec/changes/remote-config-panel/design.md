## Context

現況(讀程式碼驗證):`fleety-protocol` 的 `PROTOCOL_VERSION = 0`;`ClientMsg`/`ServerMsg` 為 `#[serde(tag="type", rename_all="snake_case")]` tagged enum;`Welcome` 已用 additive `#[serde(default)]` 欄位(server_version/audio_input/token);遠端 config 只有 `ConfigExec { target, args:Vec<String> }` → `ConfigResult { ok, output, effect, error }`(字串進、rendered 文字出)。server `conn.rs` 的 `read_client`(2588 行附近)對 `serde_json::from_str::<ClientMsg>` 失敗回 `Err`(→ 斷線);`config_apply` 用 `run_rendered`;`config.toml` save 目前非 tmp+rename+mutex(fleety-tools config.rs `save`);providers.toml 已 tmp+rename。auth-default-on 已上:`auth.required()` 可查、遠端 mutating 已在 auth 關閉時被擋。CLI 互動 `fleety config`(TTY)目前開單區 config edit;`provider edit` 開 provider_tui。

## Goals / Non-Goals

**Goals:** 結構化遠端 config 通道(Snapshot/Apply)+ revision 樂觀鎖 + secret tri-state + 能力協商 + 未知 frame 不斷線 + server config 真原子存檔;互動全包三區面板(連線/本機/server)+ 遠端互動 edit;敏感 key 授權/告警/稽核。

**Non-Goals:**（見 proposal）裝置分級、配對強化、wss/TLS 硬要求、改 Phase 1 非互動命令面。

## Decisions

### 分兩階段落地(2a wire 先、2b 面板後)

本 change 內部分兩段以降風險、可增量驗證:**2a(wire,先做且可獨立驗證)**= protocol 新 frame + server snapshot/apply handler + 原子存檔 + 能力協商 + 未知 frame 容忍;**2b(面板)**= CLI 三區互動面板 + 能力偵測走 Snapshot/Apply 或退回 ConfigExec。2a 全綠後才接 2b;2b 若過大可再切,缺口列 Notes。

### Protocol:結構化 config 通道 + 能力協商 + 未知 frame 容忍(補 M3/M4)

`fleety-protocol` 新增:`ClientMsg::ConfigSnapshot { target }`、`ClientMsg::ConfigApply { target, base_revision, changes: Vec<ConfigChange> }`;`ServerMsg::ConfigSnapshotResult { revision, entries: Vec<ConfigEntry>, providers_json }`;型別 `ConfigEntry { key, scope, value, default, description, secret, is_set, effect, choices: Vec<String> }`、`ConfigChange { key, op: ChangeOp(keep|set|clear), value: Option<String> }`。`Welcome` 加 additive `config_protocol: u32`(舊 server 送 0 / 缺省 0)。`PROTOCOL_VERSION` +1。**未知 frame 容忍**:`ClientMsg` 加 `#[serde(other)] Unknown` catch-all(或 read_client 解析失敗改回哨兵),server 收到 Unknown 回 `ServerMsg::Error { kind:"unsupported" }` 而**不斷線**——否則未來任何 additive frame 都炸連線。所有新欄位 additive、舊端可解析。

### Server:snapshot builder + apply handler(revision 樂觀鎖 + secret tri-state + 授權)

`conn.rs` 加 `ConfigSnapshot` handler:組 `ConfigSnapshotResult`——`entries` 來自 registry(每 key 的 scope/default/description/secret/is_set/effect/choices;secret 只回 `is_set` 不回值,並記稽核誰讀),`providers_json` 帶結構化 provider/model;`revision` = config.toml 內容 hash + server boot id。`ConfigApply` handler:先比對 `base_revision`,不符回 conflict(補 M3 lost-update);sparse `changes` 逐一套用,`op=keep` 略過、`set` 寫新值(先 validate)、`clear` 清除;Server-scope mutate 要求 `auth.required()`(接 auth-default-on §4);敏感 key(provider base_url/key、FLEETY_BACKUP_REPO/_TOKEN、oauth endpoint)套用前告警 + 記稽核(新舊 host)。回 `ConfigResult`。

### Server:config.toml 真原子存檔(補 M3)

fleety-tools config.rs 的 `save` 改 tmp + rename + 單一 per-file mutex(比照 `providers_config::write_providers`);壞檔不 fail-soft 退 default,回明確錯誤。加 `revision(path) -> String`(內容 hash)給樂觀鎖。

### CLI:能力偵測 + 面板(2b)

CLI 依 `Welcome.config_protocol` 決定走結構化 Snapshot/Apply 或退回舊 `ConfigExec`(前向相容,補 M4)。裸 `fleety config`(TTY)開新 `config_panel.rs`:ratatui 三區(Tab:連線 / 本機 / server)。連線區編 connections.toml(Phase 1 module)、本機區編 Cli/Shared(Phase 1 scoped)、server 區拉 `ConfigSnapshotResult` 顯示並經 `ConfigApply` 遠端套用;secret 遮罩 write-only(tri-state);provider 依 type 顯欄位;生效時機在 detail pane 標示。非 TTY 退回文字(Phase 1 命令)。

## Implementation Contract

**行為:**
- 新 CLI 對新 server:`fleety config`(TTY)開三區面板;server 區顯示 server 設定 + provider/model,編輯經 `ConfigApply` 遠端套用、`ConfigResult` 回生效時機。
- 樂觀鎖:兩端同時改,base_revision 過期的 apply 回 conflict、不覆蓋。
- secret:snapshot 只回 `is_set`;apply 的 secret change 為 tri-state,遮罩值不回寫。
- 舊 server(config_protocol=0):CLI 偵測後退回 `ConfigExec` 文字面(不炸)。
- 未知 frame:新 CLI 送 `ConfigSnapshot` 給舊 server → 舊 server 回 unsupported error(若舊 server 也升級了容忍)/ 或 CLI 因 config_protocol=0 根本不送 → 連線續存。
- server config.toml 存檔原子;壞檔回明確錯誤不退 default。
- Server-scope mutate 在 auth 關閉時被拒(已由 auth-default-on 擋;此處確保 Apply 路徑同規則);敏感 key 套用前告警 + 稽核。

**介面 / 資料形狀:**
- protocol:`ConfigSnapshot`/`ConfigApply`(ClientMsg)、`ConfigSnapshotResult`(ServerMsg)、`ConfigEntry`/`ConfigChange`/`ChangeOp`、`Welcome.config_protocol`、`ClientMsg::Unknown`、`PROTOCOL_VERSION+1`。
- fleety-tools:`config::snapshot_entries() -> Vec<ConfigEntry-ish>`、`config::revision(path) -> String`、`save` 原子化。
- fleety-server:`conn.rs` ConfigSnapshot/ConfigApply handler + Welcome config_protocol。
- fleety-cli:`config_panel.rs`(三區面板)+ 能力偵測 + Snapshot/Apply 用戶端流程。

**失敗模式:**
- base_revision 過期 → conflict 回應,不覆蓋。
- 未知 frame → unsupported error frame、連線續存。
- config.toml 壞 → 明確錯誤(save/load 不 fail-soft 退 default)。
- auth 關閉時 Server-scope Apply → unauthenticated 拒。
- 舊 server → CLI 退回 ConfigExec。

**驗收條件:**
- fleety-protocol 單元:新 frame/型別 serde round-trip、additive(舊端解析)、Unknown catch-all 解析。
- fleety-server 單元:snapshot entries(secret 只 is_set)、apply(set/clear/keep)、revision conflict 拒、原子存檔、未知 frame 回 unsupported 不斷線、Server-scope auth 閘。
- fleety-cli 單元:能力偵測分支、面板三區 state/apply round-trip(headless TestBackend)、secret tri-state。
- `cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15、`cargo clippy` 無新違規。

**範圍邊界:**
- In scope:結構化 config 通道 + 樂觀鎖 + tri-state + 能力協商 + 未知 frame 容忍 + 原子存檔 + 三區面板 + 遠端 edit + 敏感 key 授權/告警/稽核。
- Out of scope:裝置分級、配對強化、wss/TLS 硬要求、Phase 1 命令面變更。

## Risks / Trade-offs

- [wire 半套危險] PROTOCOL_VERSION +1 + 新 handler 必須成套;故內部先做 2a 全綠再接 2b,未知 frame 容忍避免升級不同步炸連線。
- [面板過大] 2b 三區面板 + 遠端 edit 大;連線/本機兩區可直接複用 Phase 1 module,server 區為新;必要時 server 區最小可用(list + 單值 edit)先交付,provider/model 完整互動列 Notes 續做。
- [樂觀鎖 revision 定義] 內容 hash + boot id;server 重啟改 boot id → 舊 revision 自然失效(避免跨重啟誤套),可接受。
- [敏感 key 稽核與 secret 讀取分級] snapshot 對 secret 只回 is_set + 記讀取者;避免低信任裝置偵察。
- [向後相容] 所有新欄位 additive + config_protocol 協商 + 未知 frame 容忍,舊端不壞。
