## Context

現況：`fleety config provider edit` 在 TTY 上由 CLI 的 config 分派攔截（is_interactive_edit 把 ["provider","edit"] 視為互動編輯 → 本機分支），開 ratatui 編輯器直接 load/save **CLI 本機**的 providers_path()。providers.toml 的消費者是 fleety-server（provider 池、model 角色）。同族的非互動子命令（provider add/set/remove、model set/…）預設 target=Server，走 `ConfigExec` 在 server 端執行 `run_providers_at(providers_path())` ——已遠端。結構化面：`ConfigSnapshot` 的回覆**已帶 `providers_json`**（server 端 ProvidersConfig 的 JSON），`ConfigApply { target, base_revision, changes }` 只承載 config key 級變更，沒有 providers 寫回通道。config revision = config.toml 內容 hash + boot id（providers.toml 不在指紋內）。config protocol 版本欄位剛因 credential frames bump 到 2（同版尚未發佈）。

三區設定面板（config_panel）的 Server 區 provider 互動編輯是註解明示的 follow-up，現指引使用者用 `config provider|model` 子命令。

## Goals / Non-Goals

**Goals:**

- `config provider edit`（預設 target）作用於連線中的 server：snapshot 取 providers → 本地互動編輯記憶體值 → apply 寫回 server，驗證與原子寫入在 server 端、語義與本機版一致。
- providers 併發編輯受 optimistic lock 保護（revision 涵蓋 providers.toml）。
- 新 CLI 對舊 server 不靜默丟失：版本閘擋在編輯器開啟之前。
- `--target local` 保留現行本機檔案編輯（顯式選擇，供 server 同機或離線情境）。

**Non-Goals:**

- 不做三區面板內嵌的完整 provider 編輯 UI（面板繼續導引到 `config provider edit`，本變更僅更新導引文字與註解）。
- 不動非互動 provider/model 子命令（已遠端，走 ConfigExec）。
- 不動 providers.toml 的格式、驗證規則、原子寫入實作（write_providers 原樣沿用）。
- 不新增 frame（複用 ConfigApply）。

## Decisions

### 決策一：ConfigApply 擴充可選 providers_json 欄位

`ConfigApply` 增加 `providers_json: Option<String>`（serde default、None 不序列化，additive）：帶值時 server 反序列化為 ProvidersConfig、跑既有驗證、以既有原子寫入落到 server 的 providers.toml；與同一 frame 內的 `changes` 同受 `base_revision` optimistic lock 保護。整份取代（不是 diff）：互動編輯器本來就以整份文件為編輯單位，與本機版 save 語義一致。

- 否決「新 frame ProvidersPut」：ConfigApply 已具備 revision 鎖與 audit 路徑，新 frame 徒增協定面。
- 否決「把編輯 diff 轉成 ConfigExec provider 子命令序列重放」：刪改並存的編輯難以無損表達成命令序列，中途失敗會留半套狀態。

### 決策二：config revision 指紋涵蓋 providers.toml

config_revision 從「config.toml 內容 hash + boot id」擴為「config.toml hash + providers.toml hash + boot id」。兩個 CLI 同時 provider edit 時，後到的 apply 因 revision 不符被 conflict 拒絕（現行 stale-apply 行為自然延伸）；providers 變更也使 config key 的過期 snapshot 失效，反之亦然——單一 revision 涵蓋整個遠端設定面，簡單且保守（誤衝突頂多重拿 snapshot）。

### 決策三：provider edit 依 target 分流

CLI 的 config 分派改為：`provider edit` 在 TTY 上依解析出的 target 走兩條路——`--target local` 顯式指定 → 現行本機 provider_tui（path 版）；預設（Server）→ 遠端流程（snapshot → 編輯 → apply）。非 TTY 不變（回子命令路徑）。遠端流程在**開編輯器之前**做版本閘（config protocol < 2 → 報「先升級 server」）——舊 server 的 serde 忽略未知欄位、會對缺 providers_json 認知的 apply 回成功，等於整份編輯靜默蒸發，必須前置擋下。

### 決策四：provider_tui 拆成值進值出

`provider_tui::run(path)` 拆為核心 `run_editor(config: ProvidersConfig) -> Result<Option<ProvidersConfig>>`（None = 使用者退出未存；Some = 存檔時的編輯結果，含編輯器內驗證）＋兩個薄 wrapper：本機版（load path → run_editor → write_providers(path)）與遠端版（snapshot 的 providers_json → run_editor → ConfigApply{providers_json}）。編輯器內部的逐欄驗證、遮蔽、確認刪除等行為不動。

- 否決「遠端流程先寫暫存檔再重用 path 版」：多一層檔案生命週期與殘留風險，值進值出更直接。

### 決策五：conflict 與失敗的呈現

apply 回 conflict（revision 不符）時，CLI 提示「server 設定在編輯期間變動，重新載入編輯器」並以新 snapshot 重開（保留使用者未存的編輯供比對過於複雜，本版直接重載——編輯 providers 的併發窗口極小）。providers_json 解析或驗證失敗（理論上編輯器已擋，server 端是第二道防線）回明確錯誤且 server 不落地。

## Implementation Contract

**行為（操作者視角）：**

- 預設：`fleety config provider edit` 在任何機器的 TTY 上編輯的是**連線中 server** 的 providers；存檔後 server 端 providers.toml 更新（原子寫入），`fleety config provider list`（遠端）立即反映。
- `fleety config --target local provider edit`：行為與現行完全一致（本機檔案）。
- 舊 server（config protocol < 2）：進編輯器之前即報「先在 server 主機 fleety update（或等 fleet 收斂）再編輯」，不開編輯器。
- 併發編輯：後存者收到 conflict 提示並重載最新內容，先存者的變更不丟失。
- 非 TTY：照舊落回子命令用法提示。

**介面與資料形狀：**

- `ClientMsg::ConfigApply` 增 `providers_json: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`），內容為 ProvidersConfig 的 JSON（與 `ConfigSnapshotResult.providers_json` 同形）。
- `provider_tui` 公開面：新 `run_editor(ProvidersConfig) -> Result<Option<ProvidersConfig>>`；既有 `run(&Path)` 保留為本機 wrapper。
- server 端 apply 語義：providers_json 存在時 —— parse 失敗 → invalid 錯誤不落地；驗證失敗 → 錯誤不落地；成功 → write_providers 原子寫入；與 changes 共用同一 revision 檢查與同一 ConfigResult 回覆。audit 事件在既有 config_apply 事件上加 providers 變更旗標，不含任何 key 值。
- config_revision：涵蓋 providers.toml 內容（檔案不存在視為空內容參與 hash）。

**失敗模式：**

- 版本閘擋下 → 錯誤含升級指引，未發送任何 frame。
- snapshot 失敗（連線/未配對）→ 沿 config_remote 既有錯誤與 remediation。
- apply conflict → CLI 重載編輯器；apply 其他失敗 → 顯示 server 錯誤，編輯器內容留在畫面供再試。
- auth 關閉的 server：providers_json 屬 mutating apply，沿既有「遠端寫入⇒認證必開」拒絕路徑。

**驗收準則：**

- fleety-protocol：ConfigApply 帶/不帶 providers_json 的 round-trip 測試；舊形（無欄位）反序列化 → None。
- fleety-server：apply providers_json 的單元測試——合法 JSON 寫入後檔案內容正確、壞 JSON 拒絕不落地、驗證失敗拒絕不落地、revision 不符 conflict 不落地、revision 涵蓋 providers.toml（改 providers 後舊 revision 的 apply 被拒）。
- fleety-cli：run_editor 值進值出的既有互動測試改造後全綠；版本閘測試（<2 拒、2 過）；target 分流測試（local → 本機路徑、預設 → 遠端路徑的分派判斷為純函式並有測試）。
- 全 workspace：cargo test、clippy -D warnings、fmt 乾淨。
- 手動端到端（發版後）：Windows CLI 對 Mac mini server 跑 config provider edit，存檔後 server 端 providers.toml 變更、模型池生效。

**範圍邊界：**

- 範圍內：crates/fleety-protocol/src/lib.rs、crates/fleety-cli/src/provider_tui.rs、crates/fleety-cli/src/config.rs、crates/fleety-cli/src/main.rs、crates/fleety-cli/src/config_panel.rs（註解與導引文字）、crates/fleety-server/src/conn.rs、docs/env.md。
- 範圍外：providers.toml 格式與驗證、非互動子命令、面板內嵌編輯 UI、fleetyd。

## Risks / Trade-offs

- [providers.toml 含 provider key，整份經連線傳輸] → 與 `config set FLEETY_MODEL_KEY`、credential put 同一已認證通道與信任邊界；非新增暴露面。snapshot 回傳的 providers_json 既已含此資料（現行為）。
- [整份取代 vs 細粒度 diff] → revision 鎖已防 lost update；整份語義與編輯器編輯單位一致。代價：conflict 時使用者重編，接受（併發編輯 providers 極罕見）。
- [revision 涵蓋 providers.toml 使 config-key apply 也可能因 providers 變動而 conflict] → 保守方向的誤衝突，重拿 snapshot 即可；比漏鎖安全。
- [舊 CLI 新 server] → 舊 CLI 不送 providers_json，行為不變；本機編輯錯位在舊 CLI 上仍存在，升級即修。

## Migration Plan

與 credential 變更同版出貨（protocol 2 能力集一次講清楚）。無資料遷移：providers.toml 格式不變。回滾 revert 即可。

## Open Questions

- 無阻斷項。
