## 1. 內建預設 manifest URL(crates/fleety-tools/src/update.rs)

- [x] 1.1 依 spec「Manifest URL templating」:新增 `DEFAULT_UPDATE_MANIFEST` 常數(專案自己的 GitHub releases latest 形式)與 `manifest_template()`(env 有則用、無則用預設);`manifest_url_for` / `manifest_is_templated` / `manifest_supports_version` 改用它,unset 時不再報錯而回內建預設。單元測試:unset → 內建 GitHub URL(含 {bin} 代換、is_templated=true、supports_version=false)、set → env 覆蓋。驗證:cargo test -p fleety-tools --lib update 全綠。

## 2. 文件與驗證

- [x] 2.1 docs/env.md 的 `FLEETY_UPDATE_MANIFEST` 說明改為「內建預設(專案 GitHub releases),env 覆蓋;daemon 無人值守輪詢仍需顯式設定」。驗證:內容審閱與 spec 用語一致。
- [x] 2.2 全驗證:cargo test -p fleety-tools、cargo clippy -p fleety-tools -- -D warnings、cargo fmt --all -- --check 乾淨。驗證:指令輸出乾淨。
