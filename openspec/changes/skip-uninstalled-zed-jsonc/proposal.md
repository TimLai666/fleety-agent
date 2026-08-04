## Why

`fleety update` 會重新整理已安裝的 Zed ACP entry，但目前在確認是否存在 Fleety entry 之前，就先把整份 Zed `settings.json` 當成純 JSON 解析。Zed 的預設設定通常是 JSONC，含有註解或尾逗號，因此即使使用者沒有安裝 Fleety ACP，更新也會被無關的 Zed 設定中止。

## What Changes

- 沒有已安裝的 `agent_servers.Fleety` entry 時，`fleety update` 直接略過 Zed 設定整理，不解析也不改寫該檔案。
- 只有已存在 Fleety entry 時，才維持目前的嚴格解析、端點驗證與安全更新行為。
- 增加 JSONC 設定且沒有 Fleety entry 的回歸測試，並保留 JSONC Fleety entry 不被安全更新的保護。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `acp-adapter`: `fleety update` 只在 Zed 已安裝 Fleety ACP entry 時要求設定可安全解析。

## Impact

- Affected specs: `acp-adapter`
- Affected code:
  - Modified: `crates/fleety-cli/src/acp.rs`
- Affected documentation:
  - Modified: `docs/acp.md`
- Affected tests:
  - Modified: `crates/fleety-cli/src/acp.rs`
