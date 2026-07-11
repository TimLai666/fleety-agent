## Why

`fleety config provider edit` 的互動編輯器直接開啟並寫回 **CLI 本機**的 `~/.fleety/providers.toml`，但 providers.toml 的消費者是 fleety-server（決定 model provider 與 model 池）。同一個指令族的其他子命令（`config provider add/set/remove`、`config model set/…`）預設 target 都已走連線作用於 server，唯獨互動編輯器落在本機——CLI 與 server 不同機時，使用者在編輯器裡存的東西 server 永遠看不到，而且部分子命令對、部分錯，比全錯更難察覺。三區設定面板的 Server 區也因此缺 provider 互動編輯（原始碼註解自承是 follow-up）。這與 config 面已完成的遠端化和「CLI 是遠端控制 server 的介面」定位相悖，與剛完成的 codex-oauth-server-side 是同一類錯位的最後一塊。

## What Changes

- `fleety config provider edit`（預設 target=server）改為遠端編輯：以 `ConfigSnapshot` 取回 server 的 providers（`providers_json` 欄位既已存在）與 revision，互動編輯器改成值進值出（編輯記憶體中的 ProvidersConfig），存檔時以 `ConfigApply` 送回 server；`--target local` 顯式指定時保留現行本機檔案編輯。
- fleety-protocol 的 `ConfigApply` 新增可選欄位 `providers_json`（additive）：帶值時 server 以既有驗證與原子寫入把整份 providers 設定寫回 providers.toml。此能力併入 config protocol 版本 2 的能力集（與 credential frames 同版出貨）。
- config revision 的指紋擴為同時涵蓋 config.toml 與 providers.toml 內容：兩個 CLI 併發編輯 providers 時，過期的 apply 被 optimistic lock 以 conflict 拒絕，不再靜默互相覆蓋。
- CLI 端版本閘：對 config protocol < 2 的 server，遠端 provider edit 在開編輯器之前即報「先升級 server」——舊 server 的 serde 會忽略未知的 `providers_json` 欄位並回報成功，等於靜默丟失整份編輯，必須擋在前面。
- server 端 `ConfigApply` handler 擴充：providers_json 解析失敗或驗證失敗即拒絕且不落地；成功寫入沿既有 config audit 路徑記錄（記「providers 已變更」，不記 key 值）。
- 三區面板 Server 區的 provider 註解與導引文字更新（完整面板內編輯仍為 follow-up，但指引改指向已遠端化的 `config provider edit`）。
- docs/env.md 與 README 對應措辭更新。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `provider-config-surface`: 互動 provider 編輯器的作用對象從本機檔案改為連線中的 server（snapshot→編輯→apply），`--target local` 保留本機行為；驗證與原子寫入語義不變、發生在 server 端。
- `structured-config-protocol`: `ConfigApply` 增加可選的整份 providers 寫回；config revision 指紋涵蓋 providers.toml，使 providers 併發編輯也受 optimistic lock 保護。

## Impact

- Affected specs: `provider-config-surface`、`structured-config-protocol`
- Affected code:
  - Modified:
    - crates/fleety-protocol/src/lib.rs
    - crates/fleety-cli/src/provider_tui.rs
    - crates/fleety-cli/src/config.rs
    - crates/fleety-cli/src/main.rs
    - crates/fleety-cli/src/config_panel.rs
    - crates/fleety-server/src/conn.rs
    - docs/env.md
  - New: （無）
  - Removed: （無）
- 相容性：`providers_json` 為 additive 可選欄位，舊 CLI 不送、行為不變；新 CLI 對舊 server 由版本閘擋下（不靜默丟失）。`--target local` 完整保留本機編輯路徑。
- 安全：providers.toml 可含 provider key——傳輸走已認證連線（與 config set 秘密值同一通道與信任邊界）；audit 不記 key 值；編輯器顯示遮蔽照舊。
