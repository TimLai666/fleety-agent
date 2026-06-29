## Why

provider-pool 引入 ~/.fleety/providers.toml(具名 provider + group + role 映射),但只做了「讀取/解析」。使用者要能用 `config` 指令與互動式設定畫面去新增/修改/刪除 provider、組 group、綁 role,而不是只能手寫 providers.toml 或設環境變數。現有 config 指令與 ratatui 畫面只處理扁平的 FLEETY_* 純量鍵(fleety_tools::config),無法管理 providers.toml 這種結構化清單。

## What Changes

- 在 fleety_tools 新增 providers.toml 的**寫入器**(序列化 ProvidersConfig 並原子寫檔;provider-pool 只做解析,本案補上寫入往返)。
- 在共用的 fleety_tools::config 新增 `provider` / `group` / `role` 子指令(三個 binary 都能用):
  - `provider add|set|remove|list`(欄位 name/base_url/model/key/stream/modalities/effort;key 顯示時遮罩)
  - `group set|remove|list`(name + members + strategy=round_robin|failover)
  - `role set|unset|list`(role 名 → provider 或 group 名)
  - 寫入前驗證:provider name 唯一、group members 必須存在、strategy 只能二選一、role 目標必須存在;違反回明確錯誤訊息(errors-as-messages,不崩潰)。
- 在 fleety-cli 的互動式 ratatui 畫面新增**providers 管理視圖**:列出 provider/group/role,可新增/編輯/刪除 provider、設定 group 成員與策略、綁定 role;沿用既有編輯緩衝區互動模式;key 遮罩顯示。

## Non-Goals

(本變更會建立 design.md,Non-Goals / 後續寫在 design 的 Goals/Non-Goals 與 Open Questions。)

## Capabilities

### New Capabilities

- `provider-config-surface`: 用 `config` 子指令(provider/group/role 的增刪改查,三個 binary 共用)與互動式 ratatui 畫面管理 ~/.fleety/providers.toml,含寫入器與寫入前驗證;不再只能靠手寫檔或環境變數。

### Modified Capabilities

(none)

## Impact

- Affected specs: provider-config-surface(新)
- Affected code:
  - New:
    - crates/fleety-cli/src/provider_tui.rs(互動式 providers 管理視圖:列出/新增/編輯/刪除 provider、group 成員與策略、role 綁定)
  - Modified:
    - crates/fleety-tools/src/config.rs(新增 provider/group/role 子指令的 parse 與處理;providers.toml 寫入器與寫入前驗證)
    - crates/fleety-cli/src/config.rs(config edit 進入點分流到 providers 管理視圖)
    - docs/env.md(config provider/group/role 子指令用法)
