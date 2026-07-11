## 1. 協定層（fleety-protocol）

- [x] 1.1 依 design「決策一：ConfigApply 擴充可選 providers_json 欄位」與 spec 的 Config changes apply atomically under optimistic locking（修改版），在 crates/fleety-protocol/src/lib.rs 為 ConfigApply 加 providers_json: Option<String>（serde default、None 不序列化），文件註解講明它屬 config protocol 2 能力集。先寫測試（tdd）：帶欄位 round-trip、舊形 JSON（無欄位）反序列化為 None、既有 structured config 測試不回歸。驗證：cargo test -p fleety-protocol 全綠。

## 2. server 端（fleety-server）

- [x] 2.1 依 design「決策二：config revision 指紋涵蓋 providers.toml」修改 crates/fleety-server/src/conn.rs 的 config_revision：納入 providers.toml 內容 hash（檔案不存在視為空）。先寫測試：改動 providers 檔後 revision 改變、config.toml 不動時亦然。驗證：cargo test -p fleety-server 全綠。
- [x] 2.2 依 spec 的 Config changes apply atomically under optimistic locking（修改版）擴充 ConfigApply handler：providers_json 存在時 parse 為 ProvidersConfig、跑既有驗證、write_providers 原子寫入；parse 或驗證失敗回錯誤且不落地；revision 不符維持 conflict 不落地；成功寫回在既有 config_apply audit 事件加 providers 變更旗標（不含 key 值）。先寫測試：合法寫回檔案內容正確、壞 JSON 不落地、驗證失敗不落地、舊 revision 的 providers apply 被 conflict 拒絕。驗證：cargo test -p fleety-server 全綠。

## 3. CLI 端（fleety-cli）

- [x] 3.1 依 design「決策四：provider_tui 拆成值進值出」重構 crates/fleety-cli/src/provider_tui.rs：核心 run_editor(ProvidersConfig) -> Result<Option<ProvidersConfig>>（None=退出未存），既有 run(&Path) 改為本機 wrapper（load → run_editor → 原子寫回），編輯器內逐欄驗證、遮蔽、刪除確認行為不變。驗證：cargo test -p fleety-cli 既有 provider_tui 測試改造後全綠。
- [x] 3.2 依 design「決策三：provider edit 依 target 分流」與 spec 的 An interactive screen manages providers on a TTY（修改版）：CLI config 分派改為 provider edit 在 TTY 上依 target 走兩路——顯式 --target local 走本機 wrapper；預設走遠端流程（ConfigSnapshot 取 providers_json 與 revision → run_editor → ConfigApply 帶 providers_json），版本閘在開編輯器之前（config protocol < 2 報升級指引，沿 auth 的既有模式）；conflict 回覆依 design「決策五：conflict 與失敗的呈現」重載編輯器。分流判斷抽為純函式。先寫測試：分流純函式（local/預設/非 TTY）、版本閘（1 拒 2 過）。驗證：cargo test -p fleety-cli 全綠。
- [x] 3.3 crates/fleety-cli/src/config_panel.rs 的 Server 區 provider 導引文字與模組註解更新：指向已遠端化的 config provider edit（面板內嵌編輯仍為 follow-up）。驗證：內容審閱，cargo test -p fleety-cli 全綠。

## 4. 文件

- [x] 4.1 [P] docs/env.md 的 provider 章節更新：講明 config provider edit 預設編輯連線中 server 的 providers、--target local 的本機行為、舊 server 需先升級。驗證：內容審閱與 spec 用語一致。

## 5. 整體驗證

- [x] 5.1 全 workspace 驗證：cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過；確認 providers.toml is written back atomically and validated 的既有行為未回歸（fleety-tools providers_config 測試全綠、write_providers 未改動）。驗證：指令輸出乾淨。
