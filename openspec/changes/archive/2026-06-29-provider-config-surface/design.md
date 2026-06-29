## Context

provider-pool(本案的前置 change)新增 ~/.fleety/providers.toml 與其解析(ProvidersConfig，含 providers/groups/roles），型別放在 crates/fleety-tools。現有設定管理在 crates/fleety-tools/src/config.rs:共用的典型登錄表(扁平 FLEETY_* 純量鍵)+ parse/run(list/get/set/unset/edit/help)+ config_path()，寫入 ~/.fleety/config.toml;三個 binary(fleety-cli/fleety-server/fleetyd)共用。fleety-cli 在 crates/fleety-cli/src/config.rs 覆寫 edit:stdout 是 TTY 時開 ratatui 畫面(逐列編輯純量值，secret 遮罩),否則走共用的逐行迴圈。本案要在這個架構上加「結構化清單(providers.toml)」的管理面。工作區規則:errors-as-messages 不崩潰、agent-core 不依賴 fleety crate(本案不碰 agent-core)。

## Goals / Non-Goals

**Goals:**

- 不靠手寫檔或環境變數,就能新增/修改/刪除 provider、組 group、綁 role。
- 子指令(scriptable)三個 binary 共用;互動式畫面(CLI/TTY)提供同等管理。
- 寫入前驗證,壞輸入回明確訊息而非寫出壞檔或崩潰。
- 既有扁平鍵 config 行為不變。

**Non-Goals:**

- 不做端點連線/金鑰有效性實測(列 Open Questions)。
- 不保留 providers.toml 的註解/排版(寫入是從模型重建,與既有 config.toml set/unset 同樣會重寫)。
- 不改 provider-pool 的執行期行為(PoolProvider/resolve);本案只動「設定的編輯面」。

## Decisions

### providers.toml 寫入器(序列化 + 原子寫)

在 crates/fleety-tools 新增 ProvidersConfig 的寫入:讓型別 derive Serialize(provider-pool 若只 derive Deserialize 則本案補上),以既有 toml 序列化成字串,寫到暫存檔再 rename 取代(原子寫,避免半寫壞檔)。round-trip:parse→model→write 後再 parse 應得相同模型。理由:結構化清單需要程式化寫回;原子寫避免並發/中斷壞檔;重建式寫入簡單,代價是丟失註解(可接受,與 config.toml 同)。

### 共用 provider/group/role 子指令 + 寫入前驗證

在 fleety_tools::config 的 parse/run 擴充三組子指令(三個 binary 共用):
- `provider add|set|remove|list`:欄位 name/base_url/model/key/stream/modalities/effort;list 時 key 遮罩。
- `group set|remove|list`:name + members(provider 名清單)+ strategy(round_robin|failover)。
- `role set|unset|list`:role 名 → provider 或 group 名。
寫入前驗證(純函式 validate(config) -> Result<()>):provider name 唯一;group members 全部存在;strategy 僅 round_robin/failover;role 目標(provider 或 group)存在。違反回 CoreError 訊息字串,絕不寫檔、不崩潰。理由:與既有共用 config 一致(三 binary 同步);驗證抽純函式可單元測試;errors-as-messages。

### CLI 互動式 providers 管理視圖(ratatui)

在 crates/fleety-cli 新增 provider_tui.rs:`config provider edit`(stdout 為 TTY 時)開 ratatui 畫面,列出 providers/groups/roles 三區;支援新增/編輯/刪除 provider(沿用既有 edit 緩衝區逐欄輸入)、設定 group 成員與 strategy、綁定 role;key 遮罩顯示;存檔時走同一個 validate + 寫入器。非 TTY 或無 `edit` → 回退共用子指令。理由:與既有 edit 畫面同模式(降低學習/實作成本);CLI-only 因 ratatui 需 TTY;存檔共用 validate/writer 確保與子指令一致。

### 與既有扁平鍵 config 的關係

providers.toml 與 config.toml 分開(沿用 provider-pool 決策)。本案只新增 provider/group/role 子指令與 provider 互動視圖,既有 list/get/set/unset/edit(扁平鍵)行為完全不變。理由:兩種設定資料形狀不同(純量 vs 清單),分開避免互相汙染。

## Implementation Contract

**行為(Behavior):**

- `config provider add foo --base-url URL --model M --key K [--stream] [--modalities text,image] [--effort medium]` → 寫入 providers.toml 多一個 provider;name 重複 → 錯誤訊息、不寫。
- `config provider set foo --model M2` → 更新既有欄位;不存在 → 錯誤。
- `config provider remove foo` → 移除;若被某 group/role 引用 → 錯誤(避免懸空引用),訊息指出引用者。
- `config provider list` → 列出所有 provider,key 遮罩。
- `config group set g --members a,b --strategy round_robin` → 新增/覆寫 group;成員不存在 → 錯誤;strategy 非二選一 → 錯誤。`group remove`/`group list` 對應。
- `config role set main g` → main→g;目標不存在 → 錯誤。`role unset main`/`role list` 對應。
- `config provider edit`(TTY)→ ratatui 畫面;存檔 = validate + 原子寫;驗證失敗顯示訊息、不寫。
- 三個 binary(fleety/fleety-server/fleetyd)都認得上述子指令。

**介面 / 資料形狀:**

- crates/fleety-tools/src/config.rs:擴充 Command 列舉與 parse 以涵蓋 provider/group/role 動詞;`fn validate_providers(cfg: &ProvidersConfig) -> Result<()>`(純);`fn write_providers(path, cfg) -> Result<()>`(原子寫);各子指令處理函式 load→mutate→validate→write。
- ProvidersConfig(fleety-tools,源自 provider-pool):本案確保 derive Serialize。
- crates/fleety-cli/src/provider_tui.rs:ratatui 畫面狀態(providers/groups/roles 列表 + 選取 + 編輯緩衝);存檔呼叫 fleety_tools::config 的 validate+write。
- crates/fleety-cli/src/config.rs:run() 在 `provider edit` 且 TTY 時分流到 provider_tui。

**失敗模式:**

- 任一驗證失敗(重名/懸空引用/壞 strategy/未知目標)→ 回 CoreError 訊息、不寫檔、結束碼非零(子指令)或顯示於畫面(TTY)。
- providers.toml 不存在 → 視為空集合,首次 add 建立檔案。
- 寫入中斷 → 原子 rename 保證舊檔完整或新檔完整,不留半檔。

**驗收標準(Acceptance):**

- 單元測試:validate_providers 對重名、group 成員缺失、壞 strategy、role 目標缺失各回 Err;合法 → Ok。
- 單元測試:write→parse round-trip 得相同模型;remove 被引用的 provider → Err 且訊息含引用者。
- 單元測試:parse 子指令(provider/group/role 動詞 + 旗標)成純函式 Command,壞旗標 → Err。
- 互動畫面:以可注入的模型驗證「新增/刪除 provider、設 group、綁 role」改動 ProvidersConfig,存檔走 validate(畫面繪製本身手動驗證)。
- clippy -D 乾淨、env 測試單執行緒;TTY 畫面與真檔寫入做手動驗證。

**範圍邊界:**

- In scope:providers.toml 寫入器 + 驗證、provider/group/role 子指令(三 binary)、CLI 互動 providers 視圖、文件。
- Out of scope:provider-pool 執行期(PoolProvider/resolve)、端點連線實測、註解保留、扁平鍵 config 既有行為。

## Risks / Trade-offs

- [重建式寫入丟失 providers.toml 註解/排版] → 與既有 config.toml set/unset 一致;文件說明「由工具管理」。
- [remove 造成懸空引用] → 寫入前驗證擋掉,訊息指出引用者;使用者需先解除引用。
- [互動畫面 + 子指令兩條路徑邏輯重複] → 兩者共用同一 validate + writer,僅 UI 不同,降低分歧。
- [本案依賴尚未 apply 的 provider-pool 型別] → apply 順序:先 provider-pool 再本案;tasks 標明前置。

## Migration Plan

- 純加層:沒人用 providers.toml 時,本案的子指令/畫面只是多出的入口,既有扁平鍵 config 不變。
- 無資料遷移。回滾:移除子指令與 provider_tui,providers.toml 仍可手寫(provider-pool 仍讀)。

## Open Questions

- 端點連線 / 金鑰有效性實測(add 時試打一次):MVP 不做。
- providers.toml 註解/排版保留(改用 toml_edit 之類):MVP 不做。
- 互動畫面與扁平鍵 edit 整合成單一多分頁畫面:MVP 維持兩個分開入口。
