## 1. 頂層選單路由與常駐提示(crates/fleety-cli/src/config_panel.rs, main.rs)

- [x] 1.1 依 design「決策一:頂層選單路由（config_panel.rs）」與「決策四:常駐提示渲染」與 spec「Bare `fleety config` opens a top-level menu with guided drill-down」「The key hints stay visible」:引入 `enum Screen {Menu, Settings, Providers, Models}`,bare `fleety config`(TTY)進 Menu;純函式 `menu_select(len, idx, key)->(idx, chosen?)` 與 `hints_for(screen)->&str`;底部改兩行(提示常駐 + status);Esc 從子 Screen 回 Menu,Settings 沿用現有三區面板。main.rs bare config dispatch 改開選單。先寫測試:menu_select 導航、hints_for 各 Screen。驗證:cargo test -p fleety-cli menu_select 全綠、cargo build -p fleety-cli 乾淨。

## 2. Providers 嚮導(crates/fleety-cli/src/provider_tui.rs)

- [x] 2.1 依 design「決策二:Providers 嚮導（provider_tui.rs）」與 spec「Guided provider and model editing」:把 `Action::AddProvider` 的逗號一行輸入改成多步嚮導 —— 選單選 type(來自 provider_types)→ 逐欄提示 name/base_url/api key(遮罩);純狀態轉移 `WizardStep`(next/back)可測;完成呼叫既有 `ed.add_provider`。刪 provider 維持選取+確認。驗證:cargo test -p fleety-cli WizardStep 全綠、cargo build 乾淨。

## 3. Models 兩層 + /models 抓取(crates/fleety-cli/src/model_picker.rs)

- [x] 3.1 依 design「決策三:Models 兩層嚮導 + /models 抓取（新 model_picker.rs）」與 spec「model selection lists the chosen provider's models」「model fetch failure degrades to manual entry」:新增 model_picker.rs —— 純函式 `parse_model_ids(&Value)->Vec<String>`(取 data[].id;空/壞 JSON→空)+ `async fetch_models(base_url,key)->Result<Vec<String>>`(GET {base_url}/models,Authorization Bearer key)+ 可搜尋清單狀態。model 角色設定:先選 provider → api 型抓 /models 可搜尋選、失敗/oauth 退手動 → 加成員 + strategy(呼叫既有 set_model)。先寫測試:parse_model_ids(含空/壞)。驗證:cargo test -p fleety-cli parse_model_ids 全綠。

## 4. FLEETY_TZ 時區選單

- [x] 4.1 依 design「決策五:FLEETY_TZ 時區選單」:Settings 編 FLEETY_TZ 時,值編輯提供可搜尋的 IANA 時區清單(chrono_tz::TZ_VARIANTS 或精選)+ 允許手打;沿用既有寫入。驗證:cargo build -p fleety-cli 乾淨、手動可從清單選時區。

## 5. 整體驗證

- [x] 5.1 全 workspace 驗證:cargo test --workspace、cargo clippy --workspace --all-targets -- -D warnings、cargo fmt --all -- --check 乾淨。驗證:指令輸出乾淨。
