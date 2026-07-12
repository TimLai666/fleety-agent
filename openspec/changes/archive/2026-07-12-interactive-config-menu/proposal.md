## Why

`fleety config` 現在對使用者不好用:bare `fleety config` 開的是「編 `FLEETY_*` 設定值」的三區面板(感覺像填變數);要設 provider/model 得跑 `fleety config provider edit`,而且輸入是「打一行逗號分隔的 name, type, base_url, key」,不是真嚮導。使用者要的是:**bare `fleety config` = 一個選單,選 Providers / Models / Settings 就進下一層介面**,provider/model 走真嚮導(選單選 type、逐欄提示、設 model 時先選 provider 再從它的 `/models` 清單選),而且**常駐提示不要被動作輸出蓋掉**(現在 status 會覆蓋掉 `q: quit` 提示)。

## What Changes

- **頂層選單**:bare `fleety config`(TTY)先開一個選單 —— `Providers` / `Models` / `Settings` / `Quit`,↑/↓ 選、Enter 進下一層、Esc 返回上層、q 離開。非 TTY 或帶子命令時維持現行行為。
- **Providers(嚮導)**:新增 provider 時先用**選單選 type**(api / oauth:codex…,來自既有 `provider_types`);api 型則逐欄提示 name → base_url → api key;沿用既有 `ProviderEditor` 的純變更方法(add/remove),只換掉「逗號一行」的輸入。
- **Models(兩層嚮導)**:設 main/cheap 角色 → **先選 provider** → 用該 provider 的 base_url+key 打 `GET {base_url}/models`(OpenAI 相容),把 model id 列成**可搜尋選單**選定;端點打不通/oauth 型 → 退回手動輸入 model id。角色為 pool,可加多個 (provider, model) 成員並設 strategy。沿用 `set_model`/`unset_model`。
- **常駐提示修正**:把「按鍵提示」與 transient「status/結果訊息」分成兩行渲染,提示(含 q: quit / Esc: back)永遠可見、不被動作輸出蓋掉。
- **時區可選**:Settings 內編 `FLEETY_TZ` 時提供常見 IANA 時區的可搜尋選單(而非純手打),對齊「跟隨裝置、也能選」的需求。

## Non-Goals (optional)

- 不改 providers.toml 結構、驗證、原子寫入(沿用 `providers_config` 與 `ProviderEditor::save`)。
- 不改非互動 CLI(`fleety config provider|model …` 子命令維持)。
- 不做「跨 provider 扁平合併的一鍵選 model」(維持兩層:先 provider 再 model)。
- 不硬濾非對話模型(embedding/tts…)—— 全列 + 搜尋,讓使用者自己挑。
- 不改 tz 的 backend 兜底(已在別的變更做成「跟隨裝置」)。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `interactive-config-panel`: bare `fleety config` 由「直接進設定面板」改為「先進頂層選單再下鑽」;新增 provider/model 的嚮導式編輯(選單選型別、逐欄提示、model 先選 provider 再從其 /models 清單選)、常駐按鍵提示、以及 FLEETY_TZ 的時區選單。

## Impact

- Affected specs: `interactive-config-panel`(modified)
- Affected code:
  - Modified:
    - crates/fleety-cli/src/config_panel.rs — 頂層選單路由 + 常駐提示渲染;Settings 區沿用現有;FLEETY_TZ 時區選單
    - crates/fleety-cli/src/provider_tui.rs — 把逗號一行輸入改成嚮導(選 type 選單、逐欄提示);model 兩層 + /models 抓取選
    - crates/fleety-cli/src/main.rs — bare `fleety config` dispatch 改為開頂層選單
  - New:
    - crates/fleety-cli/src/model_picker.rs(或併入 provider_tui)— 打 provider `/models` 抓清單 + 可搜尋選單(reqwest,退手動)
  - Removed: (none)
