## 1. Grants 撤銷與列舉（privacy.rs）

- [ ] 1.1 在 `Grants` 新增 `revoke(owner, grantee, scope: Option<&str>) -> usize`：精確比對移除符合 (grantee, scope) 的授權，`scope` 為 `None` 時移除該 grantee 的全部授權，owner 列表清空後移除 owner 鍵，回傳移除筆數。以單元測試 `revoke_removes_matching_grant`、`revoke_without_scope_removes_all_for_grantee`、`revoke_nonexistent_returns_zero` 驗證，並確認移除後 `can_access` 回傳 `Deny`。
- [ ] 1.2 在 `Grants` 新增 `grants_for(owner) -> Vec<Grant>` 列舉某 owner 現有授權（owner 不存在回空 Vec）。以單元測試驗證列出的 grantee/scope 正確、且不含其他 owner 的授權。

## 2. Storage 移除路徑（storage.rs）

- [ ] 2.1 新增 `remove_grant(owner, grantee, scope: Option<&str>) -> Result<usize>`：比照 `add_grant` 先 `validate_id`，於 `append_lock` 下 load-modify-save 呼叫 `Grants::revoke` 並回傳移除筆數；無授權時仍成功回 0 且不寫壞檔案。以單元測試驗證撤銷後重新 `grants()` 載入時 `can_access` 為 `Deny`，且與 `add_grant` 併發互不覆蓋（沿用既有 lock 測試風格）。

## 3. revoke_access 工具（privacy.rs）

- [ ] 3.1 實作 `RevokeAccess` 工具（`RiskLevel::Mutate`），參數 `grantee`（必填、trim 後非空）、`scope`（選填，省略＝全部）；guest（`user_id()` 為 `None`）回錯誤；呼叫 `storage.remove_grant`，回傳 `{ ok: true, owner, grantee, scope, removed: <n> }`。以工具層測試 `revoke_access_tool_removes_and_guards_guest` 驗證 Alice 撤銷 Bob 後 `can_access(bob, alice, ...)` 為 `Deny`，且 guest 呼叫 `is_err()`。

## 4. list_access 工具（privacy.rs）

- [ ] 4.1 實作 `ListAccess` 工具（`RiskLevel::Read`），無必填參數；guest 回 `{ grants: [] }`，真實使用者回自己 `grants_for(owner)` 的 `[{ grantee, scope }, ...]`。以工具層測試驗證授權後 `list_access` 含該筆、撤銷後不含、guest 得空清單。

## 5. 註冊與描述更新（privacy.rs / conn.rs）

- [ ] 5.1 擴充 `register_grant` 一併註冊 `RevokeAccess` 與 `ListAccess`（沿用同一 `storage`/`acting`），確認 `conn.rs:376` 呼叫點無需改動；以既有工具註冊測試或新增測試確認三個工具名（`grant_access`/`revoke_access`/`list_access`）都出現在 registry。
- [ ] 5.2 更新 `grant_access` `ToolSpec.description`，移除「revoke is not yet supported」，改述可用 `revoke_access` 收回、`list_access` 檢視；以內容審閱與 `cargo test -p fleety-server` 確認描述字串更新且測試通過。

## 6. 驗證

- [ ] 6.1 執行 `cargo test -p fleety-server privacy::` 與 storage 相關測試，確認 A user can revoke and list the grants they made 需求的四個核心行為（撤銷即時生效、無 scope 全撤、列舉、guest 受限、撤空回零）皆綠燈；`cargo clippy -p fleety-server` 無 `unwrap_used`/`expect_used` 違規。