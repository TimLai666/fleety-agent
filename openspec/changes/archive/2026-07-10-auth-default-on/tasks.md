## 1. 認證預設開

- [x] 1.1 [P] 在 crates/fleety-tools/src/config.rs 把 registry 的 `FLEETY_REQUIRE_AUTH` 預設 `"0"`→`"1"`、描述改為「預設要求 token 連線(1/0);設 0 才關」(design「FLEETY_REQUIRE_AUTH 預設翻為開」),交付「Connection authentication is required by default」的預設面。單元:`resolve("FLEETY_REQUIRE_AUTH", empty)` 值為 `"1"`、source default;validator 仍只收 0/1。
- [x] 1.2 在 crates/fleety-server/src/main.rs 把 `require_auth` 讀取由 `== Ok("1")` 改為 `!= Ok("0")`(未設即開、顯式 `0` 才關;seed_env_from_config 已先跑故 config.toml 值進 env),交付「Connection authentication is required by default」runtime 面。單元:未設 → true、`"0"` → false、`"1"` → true。

## 2. 首啟配對引導(A fresh auth-required server guides first-device pairing)

- [x] 2.1 在 crates/fleety-server/src/auth.rs 新增 `AuthStore::is_uninitialized() -> bool`(無 bootstrap token 且無任何 device token),交付「A fresh auth-required server guides first-device pairing」的判定。單元:無 bootstrap+無 token → true;有 bootstrap 或有 token → false。
- [x] 2.2 在 main.rs 建完 AuthStore 後,若 `require_auth && auth.is_uninitialized()` 則 `create_pairing()` 產碼並以顯眼 log 印出 `fleety pair <code>` 指引(產碼失敗只 warn 不阻擋開機)(design「首啟配對引導(避免 auth-required 變無法配對的磚)」),交付「A fresh auth-required server guides first-device pairing」的引導行為。以既有 auth 單元 + 手動確認 log 出現配對碼。

## 3. 遠端寫入認證閘(Mutating remote config is refused when auth is disabled)

- [x] 3.1 在 crates/fleety-server/src/conn.rs 的 `ConfigExec` handler 加前置閘:`fleety_tools::config::config_effect(&args).is_some() && !auth.required()` 時回 `ConfigResult { ok:false, error.kind="unauthenticated" }` 指引先開認證、不改設定;讀取(effect None)與 auth 開啟時照舊(design「遠端寫入 ⇒ 認證必開(auth 關閉時拒收 mutating frame)」),交付「Mutating remote config is refused when auth is disabled」。單元:auth 關閉時 mutating frame 回 unauthenticated 且不落檔、`list` 讀取放行;auth 開啟時 mutating 放行。

## 4. 驗證

- [x] 4.1 全 workspace 回歸:`cargo test --workspace` 全綠、`cargo run -p fleety-eval -- run crates/fleety-eval/goldens` 15/15 不改 goldens、`cargo clippy -p fleety-tools -p fleety-server` 無新違規;確認 server_smoke 等既有測試不因預設翻開而壞(必要時補設 FLEETY_REQUIRE_AUTH=0)。驗證:測試輸出 + 首啟 log 手動確認。
