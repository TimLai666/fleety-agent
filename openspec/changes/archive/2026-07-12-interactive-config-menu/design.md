## Context

現況(讀過的結構):

- `crates/fleety-cli/src/config_panel.rs`(717 行):`App` + `enum Region {Connection, Local, Server}` 三區 Tab 面板,`run()` 是 ratatui 事件迴圈,底部一行 `status`(按鍵提示與結果訊息共用同一行 → 動作有輸出就把 `q: quit` 提示蓋掉)。
- `crates/fleety-cli/src/provider_tui.rs`(595 行):`App { ed: ProviderEditor, sel, mode: Browse|Input, status, save_now, quit }`;`Input` 模式收「一行逗號分隔」,`submit` 用 `split(',')` 解析。`ProviderEditor` 的變更方法(`add_provider`/`remove_provider`/`set_model`/`unset_model`)是**純的、可重用**;`save` 做驗證 + 原子寫入。
- provider 型別來自 `providers_config::provider_types`(api 需 base_url、oauth 不帶);Provider 有 `base_url: Option<String>` 與 key。
- bare `fleety config`(TTY)目前直接進 config_panel;CLI 有 reqwest(更新器在用)。

## Goals / Non-Goals

**Goals:** bare `fleety config` 先開頂層選單(Providers/Models/Settings/Quit)再下鑽;provider/model 走嚮導(選單選型別、逐欄提示、model 兩層 + /models 選單);常駐提示不被蓋;FLEETY_TZ 可選。

**Non-Goals:** 不改 providers.toml 結構/驗證/寫入;不改非互動子命令;不做跨 provider 扁平 model 選單;不硬濾非對話模型。

## Decisions

### 決策一:頂層選單路由（config_panel.rs）

引入 `enum Screen { Menu, Settings, Providers, Models }` 作為頂層狀態機。bare `fleety config`（TTY）進 `Screen::Menu`：一個垂直清單 `["Providers", "Models", "Settings", "Quit"]`，↑/↓ 移動、Enter 進對應 Screen、q 離開。各 Screen 的 render 與按鍵各自處理；`Esc` 從子 Screen 回 `Menu`（不直接退出程式）。`Settings` 沿用現有三區面板邏輯（把它包成一個 Screen）。非 TTY / 帶子命令維持現行行為。以純函式 `menu_select(items, idx, key)->(new_idx, chosen?)` 讓導航可測。

### 決策二:Providers 嚮導（provider_tui.rs）

把「一行逗號分隔」的 `Action::AddProvider` 換成多步 `Wizard`：
1. 選單選 type（來自 `provider_types()`）。
2. 逐欄提示：name → （type 需要則）base_url → api key（遮罩顯示）。
3. 每步 Enter 前進、Esc 回上一步/取消；完成後呼叫既有 `ed.add_provider(...)`。
刪除 provider 維持選取 + 確認。純狀態轉移抽 `WizardStep`（enum + next/back）便於測試。

### 決策三:Models 兩層嚮導 + /models 抓取（新 model_picker.rs）

設 main/cheap 角色時：
1. **選 provider**（從已設好的 provider 清單選）。
2. 若該 provider 是 api 型且有 base_url：以 `reqwest` 打 `GET {base_url}/models`（帶 key 的 Authorization），解析 `data[].id` 成清單 → **可搜尋選單**（打字過濾）→ 選定成員。抓不到/oauth 型 → 退回手動輸入 model id。
3. 可重複加多個 (provider, model) 成員，設 strategy，呼叫既有 `ed.set_model(role, members, strategy)`。
`model_picker`：`async fn fetch_models(base_url, key) -> Result<Vec<String>>`（純解析部分抽 `parse_model_ids(json)->Vec<String>` 可測）+ 可搜尋清單狀態。網路失敗回可讀錯誤、不 panic、UI 退手動。

### 決策四:常駐提示渲染

底部改成**兩行**:上行固定「按鍵提示」（依當前 Screen 給 `Enter/↑↓/Esc/q/...`），下行才是 transient `status`（結果/錯誤）。提示不再被動作輸出覆蓋。抽純函式 `hints_for(screen)->&str`。

### 決策五:FLEETY_TZ 時區選單

Settings 編 `FLEETY_TZ` 時，值編輯改為**可搜尋的常見 IANA 時區清單**（用 `chrono_tz::TZ_VARIANTS` 或精選清單），也允許直接手打。沿用既有 config set 寫入。

## Implementation Contract

**Behavior:**

- `fleety config`（TTY，無子命令）→ 頂層選單 → 選 Providers/Models/Settings 進下一層；Esc 返回、q 離開。
- Providers：新增走「選 type → 逐欄填」；Models：新增成員走「選 provider →（api）從 /models 選 / 退手動」。
- 提示列常駐可見（含 q/Esc）。FLEETY_TZ 可從時區清單選。
- 存檔沿用既有驗證 + 原子寫入；抓 model 失敗不阻斷、退手動。

**Interface / data shape:**

- `enum Screen`；純函式 `menu_select`、`hints_for(screen)`、`parse_model_ids(&Value)->Vec<String>`、provider `WizardStep` 轉移。
- `async fn fetch_models(base_url:&str, key:Option<&str>)->Result<Vec<String>>`（GET {base_url}/models）。

**Failure modes:**

- /models 打不通/非 200/非 JSON/oauth 型 → 退回手動輸入 model id（UI 提示原因）。
- 非 TTY → 不進選單（現行行為）。

**Acceptance criteria:**

- 單元測試:`menu_select` 導航（wrap/選定）、`hints_for` 各 Screen、`parse_model_ids` 解析（含空/壞 JSON）、provider WizardStep next/back。
- 手動:`fleety config` 進選單 → 加 provider（選 type + 逐欄）→ 設 model（選 provider → 選/手打 model）→ 存檔生效;提示常駐;FLEETY_TZ 可選。
- `cargo test --workspace`、`clippy -D warnings`、`fmt --check` 乾淨。

**Scope boundaries:**

- In:config_panel.rs 路由+提示+tz 選單、provider_tui.rs 嚮導、model_picker.rs、main.rs dispatch、interactive-config-panel spec。
- Out:providers.toml 結構/驗證/寫入、非互動子命令、扁平 model 選單、非對話模型過濾。

## Risks / Trade-offs

- **TUI 改動面大**:以純函式（menu_select/hints_for/parse_model_ids/WizardStep）+ 重用 `ProviderEditor` 純變更方法降低風險與提升可測性;render/事件迴圈為薄殼。
- **/models 端點不一致**:各 provider 回應格式可能不同 → 只取標準 `data[].id`，其餘退手動。
- **config 當下的網路呼叫**:抓 model 需 key + 網路 → 失敗 graceful 退手動，不阻斷設定。
- **時區清單很長**:用可搜尋過濾。
