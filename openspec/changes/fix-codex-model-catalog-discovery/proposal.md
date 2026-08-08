## Why

Fleety 目前把自身的套件版本送成 Codex 模型目錄的 `client_version`，混合了兩個互不相干的版本領域；當上游依最低 Codex 用戶端版本篩選模型時，合法回應可能被篩成空清單。現有錯誤又把缺少 `models`、空陣列與無法解析的項目全部顯示成「no model IDs」，無法判斷真正失敗點。

## What Changes

- Codex 模型目錄請求改用獨立、明確維護的 Codex 目錄相容版本，不再重用 Fleety／`agent-core` 套件版本。
- 維持目前的伺服器端 OAuth 憑證、`GET /models` 與動態 `slug`／`id` 解析，不硬編碼模型名稱，也不要求安裝本機 Codex CLI。
- 對成功但不可用的回應區分「缺少 models 陣列」「models 為空」「項目存在但沒有可用 ID」，回傳經過清理且不含憑證的診斷。
- Provider 畫面的 `catalog` 狀態改用不會誤解成已成功載入的文字；只有實際取得非空模型清單才視為目錄載入成功。
- 增加受控目錄回應、最低用戶端版本篩選、敏感資訊遮蔽與手動輸入 fallback 的回歸測試。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `oauth-provider-model-discovery`: Codex 目錄請求使用獨立的相容版本，成功回應的不可用狀態具有可診斷且不洩密的結果，畫面不再把「可查詢」誤標成「已載入」。

## Impact

- Affected specs: `oauth-provider-model-discovery`
- Affected code:
  - Modified: `crates/fleety-tools/src/oauth.rs`
  - Modified: `crates/fleety-cli/src/provider_tui.rs`
  - Tests: OAuth catalog unit tests and provider TUI tests
- External contract: authenticated Codex backend `GET /models`; no new dependency, config key, protocol version, or public API
