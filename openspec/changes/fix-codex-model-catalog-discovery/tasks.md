## 1. 契約與失敗測試

- [x] 1.1 依「Separate Codex catalog compatibility from Fleety versioning」核對目前官方 Codex 發行版與 `/models?client_version=...` 實作，記錄受支援的相容版本來源，並以 request fixture 先證明 `agent_core::VERSION` 會被最低版本條件篩掉而獨立版本可取得模型。
- [x] [P] 1.2 依「Classify successful catalog failures before flattening IDs」為缺少或型別錯誤的 `models`、空陣列、非空但無可用 ID、ordered de-duplication 與敏感資料遮蔽建立失敗測試，並以 scoped `cargo test` 證明各類結果目前尚未被區分。
- [x] [P] 1.3 依「Distinguish queryable and loaded catalog states」為 `catalog=Queryable`、非空結果後的 loaded 狀態及失敗後 manual fallback 建立 TUI 失敗測試，並以 scoped `cargo test` 證明 `catalog=Ready` 的既有呈現不符合契約。

## 2. Codex 目錄實作

- [x] 2.1 依「The server exposes authenticated provider model discovery」與「Keep direct authenticated catalog discovery」加入私有 Codex 目錄相容版本，讓 OAuth provider 的 `GET /models` 不再以 Fleety 套件版本決定模型資格，並以 1.1 的版本門檻 fixture 及既有 bearer/account header 測試通過驗證。
- [x] 2.2 依「Classify successful catalog failures before flattening IDs」實作三種成功但不可用回應的獨立清理診斷，保留動態 `slug`／`id`、來源順序、去重與不洩密契約，並以 1.2 的測試全數通過驗證。
- [x] 2.3 依「Catalog status distinguishes queryability from loaded data」實作 Queryable、loaded 與 fallback 呈現，避免未抓取前宣稱 Ready，並以 1.3 的 TUI 測試及 render assertion 通過驗證。

## 3. 上游邊界與回歸驗證

- [x] 3.1 依「Verify the upstream boundary with a controlled contract fixture」執行 Codex 目錄 scoped tests，確認路徑、query、驗證標頭、版本門檻、解析分類及 redaction 全部符合 fixture 契約。
- [x] 3.2 執行 API-provider discovery、舊協定 fallback、manual model-ID entry 與 provider editor scoped tests，確認本變更未改動 API provider、公用協定、設定格式或無 Codex CLI 的執行能力。
- [x] 3.3 執行 `cargo fmt --all -- --check`、相關 crates 的 scoped tests 與 `cargo clippy -p fleety-tools -p fleety-cli --all-targets -- -D warnings`，確認格式、行為與 lint gate 全部通過。
