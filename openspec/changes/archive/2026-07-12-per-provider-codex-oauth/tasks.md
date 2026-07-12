## 1. Protocol: provider 維度 + config_protocol 3(crates/fleety-protocol/src/lib.rs)

- [x] 1.1 依 design「決策一:憑證以 provider 名為 key(protocol 2 to 3)」與 spec「Credential capability is version-negotiated」:在 `CredentialPut`/`CredentialStatus`/`CredentialDelete` 加 `provider: Option<String>`(serde default None);把 `config_protocol` 常數從 2 提到 3;更新兩個 smoke-test 建構子(crates/fleety-cli/tests/cli_smoke.rs、crates/fleety-daemon/tests/fleetyd_smoke.rs)與每個建這些 frame 的地方使其編譯。先寫測試:序列化/反序列化 round-trip 對 `provider=Some("p")` 與 `provider=None`(缺欄位)都成立。驗證:cargo test -p fleety-protocol 全綠、cargo build --workspace 乾淨。

## 2. oauth per-provider 路徑 + 舊全域清除(crates/fleety-tools/src/oauth.rs)

- [x] 2.1 依 design「決策二:token 儲存在 server,per-provider(client 不落地)」與「決策三:清除舊全域憑證(不遷移)」與 spec「OAuth tokens are stored protected and refreshed automatically」:新增 `token_path_for(provider: &str) -> PathBuf`(`<root>/codex-oauth/<provider>.json`,目錄 0700 / 檔 0600;`FLEETY_CODEX_TOKENS` 仍可覆寫為單一路徑)與 `clear_legacy_global(path: &Path)`(冪等刪除舊全域檔)。先寫測試:`token_path_for("a")` 與 `token_path_for("b")` 路徑不同且落在 codex-oauth 子目錄;`clear_legacy_global` 對不存在的檔是 no-op。驗證:cargo test -p fleety-tools oauth 全綠。

## 3. Server 憑證存放 per-provider + 拒絕缺 provider + 開機清舊(crates/fleety-server/src/conn.rs)

- [x] 3.1 依 design「決策五:server 憑證存放 + oauth provider 解析(conn.rs / providers.rs)」與「決策三」「決策七:back-compat 與錯誤路徑」與 spec「Credential delivery frames」:`credential_put`/`credential_status`/`credential_delete` 改吃 `provider: Option<String>`,以 `oauth::token_path_for(name)` 解析每 provider 檔;`kind=="codex-oauth"` 而 `provider=None` → 回可操作錯誤(叫使用者更新 CLI 並 per-provider 登入),不寫入;server 啟動時 `clear_legacy_global` 刪舊全域檔。先寫測試:兩個 provider 的 put/status/delete 互相隔離(刪一個另一個仍在);缺 provider 的 codex frame 被拒且未建檔。驗證:cargo test -p fleety-server credential 全綠。

## 4. Server oauth:codex provider 以自身名稱取憑證(crates/fleety-server/src/providers.rs)

- [x] 4.1 依 design「決策五:server 憑證存放 + oauth provider 解析(conn.rs / providers.rs)」與 spec「A provider can authenticate with the OAuth token」:`build_codex_provider` 用該 provider 自身名稱經 `oauth::token_path_for(name)` 取 token,使 provider `tingzhen-codex` 讀 `codex-oauth/tingzhen-codex.json`,不再共用全域;登出/未登入該 provider 時 model 呼叫回可操作錯誤。驗證:cargo build -p fleety-server 乾淨;單元或既有 provider 測試涵蓋「用自身 provider 名解析 token 路徑」。

## 5. CLI auth per-provider(crates/fleety-cli/src/auth.rs, main.rs)

- [x] 5.1 依 design「決策四:CLI auth per-provider」與「決策七:back-compat 與錯誤路徑」與 spec「ChatGPT login uses a PKCE authorization-code flow」「Login status and logout do not leak tokens」:`auth::run` 改為 `login <provider> [--no-browser]` / `logout <provider>` / `status [<provider>]`;login/logout 前檢查 server `config_protocol >= 3`(否則報「更新 server」不開瀏覽器)、且驗證 `<provider>` 存在且為 `oauth:codex`(否則 by-name 錯誤);login 交付帶 `provider` 的 CredentialPut、不本機存;`status` 無參數時列舉連線 server 的每個 `oauth:codex` provider 各印一行。先寫測試:純參數解析(缺 provider → usage 錯誤、`--no-browser` 旗標);provider 驗證(不存在/非 oauth → by-name 錯誤)。驗證:cargo test -p fleety-cli auth 全綠。

## 6. Provider 編輯器:提示常駐(全螢幕每個畫面)(crates/fleety-cli/src/provider_tui.rs)

- [x] 6.1 依 design「決策六:provider 編輯器 — 常駐提示 + 編輯 + OAuth 動作」與 spec「The key hints stay visible」:footer 拆成常駐按鍵提示列 + status 列,`added provider 'X'` 之類只落在 status 列、永不蓋提示;Browse / AddWizard 各步驟 / ModelWizard 各步驟 / 編輯流程各自提供提示文字。先寫測試:純函式 `hint_for`(或等效)對每個畫面回非空提示;render 測試確認提示與 "added provider" 同時可見。驗證:cargo test -p fleety-cli provider_tui 全綠。

## 7. Provider 編輯器:編輯既有 provider(crates/fleety-cli/src/provider_tui.rs)

- [x] 7.1 依 design「決策六:provider 編輯器 — 常駐提示 + 編輯 + OAuth 動作」與 spec「Guided provider and model editing」:Browse 加 `e`;api 型 → 預填 base_url + 遮罩 key 的編輯嚮導,存檔呼叫新的 `ProviderEditor::set_provider(name, kind, base_url, key)`(upsert,取代既有、無新增時的重名守衛);oauth 型 → 開 OAuth 動作子選單(登入/登出/換帳號)。先寫測試:`set_provider` upsert 覆寫既有欄位;oauth 子選單導航(純狀態轉移)。驗證:cargo test -p fleety-cli provider_tui 全綠。

## 8. Provider 編輯器:OAuth 動作外送 + run_providers 改 async(crates/fleety-cli/src/provider_tui.rs, config_panel.rs)

- [x] 8.1 依 design「決策六:provider 編輯器 — 常駐提示 + 編輯 + OAuth 動作」與 spec「Guided provider and model editing」的「an oauth action leaves and re-enters the editor」scenario:選 oauth provider 的登入/登出/換帳號 → 存檔並帶 `AuthRequest { action, provider }` 離開 TUI;`provider_tui::run` 改回傳 `Result<Option<AuthRequest>>`;`config_panel::run_providers` 改 `async`,在 `ratatui::restore()` 後跑 `crate::auth::login/logout`(Switch=logout+login)並印結果,再重開編輯器;更新其呼叫點為 await。先寫測試:`AuthAction`(Login/Logout/Switch)子選單導航純轉移;完成時產生正確 `AuthRequest`。驗證:cargo test -p fleety-cli 全綠、cargo build -p fleety-cli 乾淨。

## 9. 文件同步(README.md, docs/env.md, docs/design-cli-config.md)

- [x] 9.1 依 design「決策四」「決策六」與變更完整性規則:README + docs/env.md 更新 `fleety auth login|logout|status <provider>` 為 per-provider、每個 `oauth:codex` provider 各自帳號、升級清舊全域需重登;docs/design-cli-config.md 更新 provider 編輯器可編輯既有 provider 與 oauth 登入/登出/換帳號流程。驗證:grep 確認三份文件不再描述全域單一 codex 登入。

## 10. 整體驗證

- [x] 10.1 全 workspace 驗證:cargo test --workspace、cargo clippy --workspace --all-targets -- -D warnings、cargo fmt --all -- --check 乾淨。驗證:指令輸出乾淨(ws_liveness 之類 flaky 若偶發,隔離重跑確認)。
