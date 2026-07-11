## Why

自我更新機制（fleety update、fleetyd update、24 小時輪詢、伺服器版本收斂）程式碼與單元測試都已就位，但端到端無法運作：release workflow 只發佈壓縮檔，沒有任何管道產生更新 manifest，`FLEETY_UPDATE_MANIFEST` 無處可指；updater 又是把下載位元組直接寫成執行檔、不做解壓，所以既有壓縮檔資產格式也接不上。同時 URL 模板有一個設計矛盾：伺服器版本收斂要求模板含 `{version}`，但 latest 解析（輪詢與 update 指令共用）只替換 `{bin}`，一旦照文件範例設了 `{version}` 模板，每日輪詢與 update 指令會去抓含字面 `{version}` 的網址而全數失敗。目前 GitHub 上 0 release、0 tag，趁第一個 release 之前把發佈管線與解析規則一次補齊。

## What Changes

- release workflow 為 fleety、fleety-server、fleetyd 的每個 target 加發裸二進位資產（比照 fleety-insyra 的裸資產慣例），並新增 fan-in job：計算各裸資產的 sha256，為每個 binary 產生一份多 target 的 manifest JSON（資產名為 fleety-manifest.json、fleety-server-manifest.json、fleetyd-manifest.json）附掛到 release；發佈前檢查 tag 版本與 workspace 版本一致，不一致即失敗。
- manifest schema 擴充為多 target 形式（version 加 targets map，每個 target triple 各有 url 與 sha256），另含 versioned_manifest URL 模板欄位；平面舊格式（version、url、sha256）繼續相容。updater 依自身 target triple 選取項目，manifest 沒有本平台項目時安裝報明確錯誤，但版本探測（notify 路徑）仍可用。
- latest URL 解析補全：latest 解析除替換 `{bin}` 外，將 `{version}` 替換為固定字 latest，解掉「設了 {version} 模板就弄壞輪詢」的互斥；自架伺服器以 latest 別名目錄承接。
- 伺服器版本收斂的解析鏈重整：env 模板含 `{version}` 時維持現行為；否則抓 latest manifest，版本等於目標版本即直接採用，不等則依其 versioned_manifest 模板取得釘選 manifest（取回後驗證版本欄位與目標相符，不符拒用）；都不可行時警告並跳過。
- 收斂的 sibling 更新加上 `{bin}` 模板防護：env 模板沒有 `{bin}` 時跳過 sibling 並警告。現行程式碼在「有 {version}、無 {bin}」的設定組合下，會把 fleetyd 自己的 manifest 套用到 fleety 與 fleety-server 的執行檔路徑，以錯誤的 binary 內容覆寫它們；fleety update 的 CLI 端已有這個防護，收斂端補齊。
- docs/env.md 與 docs/STATUS.md 更新：GitHub 直連的建議設定值（releases/latest/download 形式）、新 schema、解析規則、自架伺服器的 latest 別名慣例。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `self-update`: 更新 manifest 的 schema 擴充（多 target 加 versioned_manifest 欄位）、URL 模板解析規則補全（latest 解析時 {version} 替換為 latest；sibling 更新必須有 {bin} 模板）、伺服器版本收斂的版本解析鏈，以及 release 發佈更新資產與 manifest 的要求。

## Impact

- Affected specs: `self-update`
- Affected code:
  - New: （無新檔案；fan-in job 寫在既有 workflow 檔內）
  - Modified:
    - .github/workflows/release.yml
    - crates/fleety-tools/src/update.rs
    - crates/fleety-tools/src/deps.rs
    - crates/fleety-tools/src/deps/insyra.rs
    - crates/fleety-daemon/src/main.rs
    - docs/env.md
    - docs/STATUS.md
  - Removed: （無）
- 相依性：不新增任何套件；sha256 計算沿用 workflow runner 內建工具與既有 sha2 crate。
- 相容性：manifest 平面舊格式與純 `{bin}` 模板、無模板純 URL 的既有行為全部保留；`FLEETY_UPDATE_MANIFEST` 未設時一律不輪詢的 opt-in 姿態不變；forward-only（裝置永不自動降版）不變。
