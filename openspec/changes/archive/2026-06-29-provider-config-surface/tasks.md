> 前置:本 change 依賴 provider-pool(providers.toml 與 ProvidersConfig 型別)。apply 順序為先 provider-pool 再本案。

## 1. providers.toml 寫入器與驗證(fleety-tools，純函式優先)

- [x] 1.1 確保 ProvidersConfig(crates/fleety-tools，源自 provider-pool)derive Serialize;新增 `write_providers(path, cfg) -> Result<()>`:toml 序列化後寫暫存檔再 rename(原子寫),交付 "providers.toml is written back atomically and validated" 的寫入面;對應設計「providers.toml 寫入器(序列化 + 原子寫)」。先寫失敗測試:write→parse round-trip 得相同模型(含 providers/group/roles)。
- [x] 1.2 [P] 新增純函式 `validate_providers(cfg) -> Result<()>`:拒絕 provider 重名、group 成員未定義、strategy 非 round_robin/failover、role 目標未定義,訊息標出問題項;合法回 Ok,交付 "Validation rejects inconsistent provider configuration";對應設計「共用 provider/group/role 子指令 + 寫入前驗證」。先寫失敗測試:四種違反各回 Err 且訊息含項目名;合法 → Ok。

## 2. config 子指令(fleety-tools，三 binary 共用)

- [x] 2.1 在 crates/fleety-tools/src/config.rs 擴充 Command 列舉與 parse,涵蓋 `provider add|set|remove|list`、`group set|remove|list`、`role set|unset|list` 的動詞與旗標(name/base_url/model/key/stream/modalities/effort、members、strategy、role 目標),壞旗標/動詞回 Err,交付 "config subcommands manage providers, groups, and roles" 的解析面;對應設計「共用 provider/group/role 子指令 + 寫入前驗證」。先寫失敗測試:各動詞 + 旗標解析成預期 Command;未知旗標/動詞 → Err。
- [x] 2.2 實作各子指令處理:load providers.toml → mutate → `validate_providers` → `write_providers`;provider list 遮罩 key;remove 被 group/role 引用的 provider → Err 並標出引用者;providers.toml 不存在時視為空集合、首次 add 建檔,交付 "config subcommands manage providers, groups, and roles" 的行為面;對應設計「共用 provider/group/role 子指令 + 寫入前驗證」。先寫失敗測試:add→list(key 遮罩)、重名 add → Err 且檔案不變、remove 被引用 → Err 含引用者。

## 3. CLI 互動式 providers 視圖(fleety-cli，TTY)

- [x] 3.1 在 crates/fleety-cli 新增 provider_tui.rs:ratatui 畫面,列出 providers/groups/roles,支援新增/編輯/刪除 provider(逐欄編輯緩衝,沿用既有 config edit 模式)、設定 group 成員與 strategy、綁定 role;key 遮罩;存檔呼叫 fleety_tools::config 的 `validate_providers` + `write_providers`,驗證失敗顯示訊息不寫,交付 "An interactive screen manages providers on a TTY" 的畫面與存檔面;對應設計「CLI 互動式 providers 管理視圖(ratatui)」。先寫失敗測試:以可注入模型驗證新增/刪除 provider、設 group、綁 role 改動 ProvidersConfig 且存檔走 validate(繪製手動驗證)。
- [x] 3.2 在 crates/fleety-cli/src/config.rs 的 run() 分流:`config provider edit` 且 stdout 為 TTY → 開 provider_tui;否則回退共用子指令;既有扁平鍵 list/get/set/unset/edit 行為不變,交付 "An interactive screen manages providers on a TTY" 的分流/回退面;對應設計「CLI 互動式 providers 管理視圖(ratatui)」與「與既有扁平鍵 config 的關係」(既有扁平鍵 config 行為不變)。驗證:非 TTY 下 `provider edit` 走子指令路徑(整合測試/手動);既有 config 測試全綠。

## 4. 文件

- [x] 4.1 [P] 在 docs/env.md(或 config 文件)記錄 `config provider|group|role` 子指令用法、互動式 `config provider edit`、與 providers.toml/扁平鍵 config 的關係,交付 "config subcommands manage providers, groups, and roles" 的文件面。驗證:內容審查涵蓋三組動詞、strategy 兩值、key 遮罩、TTY 互動與非 TTY 回退。
