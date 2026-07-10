## Why

`grant_access` 能把個資分享給別人，但沒有任何撤銷路徑：`Grants` 只有 `grant()`、`Storage` 只有 `add_grant`，權一旦給出就永遠收不回，使用者也無從得知自己目前給了誰哪些存取。

## What Changes

- 在 `Grants` 上新增撤銷與列舉方法：`revoke(owner, grantee, scope: Option<&str>) -> usize`（回傳實際移除筆數，指定 scope 為精確比對、省略 scope 則移除該 grantee 的全部授權，清空後移除 owner 鍵）與 `grants_for(owner) -> Vec<Grant>`（列舉某 owner 現有授權）。
- 在 `Storage` 新增 `remove_grant(owner, grantee, scope: Option<&str>) -> Result<usize>`，比照 `add_grant` 在 `append_lock` 下 load-modify-save，避免併發互相覆蓋。
- 新增 `revoke_access` 工具（`RiskLevel::Mutate`）：資料 owner 收回先前授權，參數 `grantee`（必填）、`scope`（選填，省略＝收回給該 grantee 的全部授權）；guest 不可撤銷；回傳移除筆數。撤銷即時生效（`can_access` 下一次查詢即拒絕）。
- 新增 `list_access` 工具（`RiskLevel::Read`）：列出目前使用者自己給出的所有授權（grantee + scope），供使用者確認要收回什麼；guest 得到空清單。
- `register_grant` 一併註冊 `revoke_access` 與 `list_access`，維持與 `grant_access` 相同的 acting-user 綁定；`conn.rs` 呼叫點不需改動。
- 更新 `grant_access` 描述，移除「revoke is not yet supported」字樣，改為指向 `revoke_access`/`list_access`。
- 撤銷與授權都是 Mutate/Read 工具，經既有 turn 稽核管線記錄，不另建稽核儲存。

## Non-Goals

- 不改動 `can_access` 判斷邏輯與 grant 資料模型（`Grant { grantee, scope }`、`grants.json` 格式不變）。
- 不支援「以精確 scope 收窄既有 `*` 萬用授權」：撤銷採精確比對，`scope="trip"` 不會動到既有的 `*` 授權（移除 0 筆），需先撤 `*` 再重授窄 scope。
- 不新增獨立的授權稽核檔或授權變更通知/回溯撤銷（既有 turn 稽核已涵蓋工具呼叫紀錄）。
- 不處理跨裝置授權同步或授權到期（TTL），維持現行純檔案 grants 模型。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- privacy-isolation: 新增「owner 可撤銷與列舉自己給出的跨使用者授權」的需求，補上 grant 之外的 revoke 與 list 路徑。

## Impact

- Affected specs: privacy-isolation
- Affected code:
  - Modified: crates/fleety-server/src/privacy.rs
  - Modified: crates/fleety-server/src/storage.rs