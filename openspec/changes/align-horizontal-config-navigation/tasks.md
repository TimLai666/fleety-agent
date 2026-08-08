## 1. 測試先行

- [x] 1.1 以失敗測試定義「Horizontal choices follow their visual direction」：四個橫向選擇器的 Left/Right、Up/Down 相容別名、兩端不換行與 Left/Right 提示均須可觀察，並以 scoped `cargo test` 證明測試在實作前會失敗。
- [x] 1.2 以回歸測試鎖定「Preserve vertical-list navigation」：代表性的縱向清單維持 Up/Down，Left/Right 不改變選取，並以 scoped `cargo test` 驗證測試可重現既有行為。

## 2. 導覽契約實作

- [x] 2.1 依「Use layout direction as the primary navigation contract」與「Update all four horizontal choice surfaces together」實作四個橫向選擇器：Left/Up 前移、Right/Down 後移且維持既有邊界，並以 1.1 的測試全數通過驗證。
- [x] 2.2 依「Show primary keys while retaining silent aliases」更新四個橫向選擇器的固定提示為 Left/Right，保留 Up/Down 相容行為，並以提示快照或 render assertion 驗證畫面不再把 Up/Down 顯示成主要操作。

## 3. 範圍與品質驗證

- [x] 3.1 驗證「Preserve vertical-list navigation」及 Implementation Contract 的範圍邊界：執行 provider TUI scoped tests，確認縱向清單、Enter、Esc 與動作快捷鍵未改變。
- [x] 3.2 執行 `cargo fmt --all -- --check`、相關 crate 的 scoped tests 與 `cargo clippy -p fleety-cli --all-targets -- -D warnings`，確認格式、行為與 lint gate 全部通過。
