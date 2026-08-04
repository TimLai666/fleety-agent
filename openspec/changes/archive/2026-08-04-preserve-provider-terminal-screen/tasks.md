## 1. 建立可失敗的終端回歸測試

- [x] 1.1 先新增 caller-owned terminal 的 RED 測試：Provider editor 從 Browse 收到 `a` 後必須在同一個 backend 繪出 `add provider` type picker；以指定測試名稱單獨執行並確認修正前失敗。
- [x] 1.2 先新增生命週期 RED 測試：正常 Settings → Providers & Models → Enter → `a` 交接不得產生 LeaveAlternateScreen 後接 EnterAlternateScreen，且準備失敗仍保留 Settings terminal；以終端控制序列斷言確認修正前失敗。

## 2. 修正 terminal ownership

- [x] 2.1 實作設計決策「Settings passes its existing terminal into the Provider editor」：把 Provider editor 拆成 caller-owned terminal 核心入口與 standalone init/restore wrapper，使 Add Provider 在原 backend 繪製；以 1.1 測試轉綠及既有 `provider_tui` 測試驗證。
- [x] 2.2 將 Settings 的既有 terminal 沿 `provider_edit_remote_on_target` 呼叫鏈傳入 Provider 核心，移除一般 Enter／返回路徑的 restore/re-init，並確保連線、identity、snapshot 準備失敗仍在原 Settings terminal 顯示；以 1.2 測試與失敗注入測試驗證。
- [x] 2.3 實作設計決策「Plain-terminal transitions stay explicit and exceptional」：只有 OAuth action 成對執行 restore/init，成功或失敗返回後都能繼續繪製；以 OAuth terminal suspend/resume 測試驗證，並確認 standalone `fleety provider edit` 仍自行擁有 terminal。

## 3. 完整驗證終端工作區契約

- [x] 3.1 依「Regression coverage observes terminal ownership, not only route state」執行實際 PTY 重播 Tab 至 Providers & Models、Enter、`a`，確認 Add Provider 畫面出現且控制序列不含一般交接的 `CSI ? 1049 l` → `CSI ? 1049 h`；記錄命令與結果供審查。
- [x] 3.2 驗證「Interactive entry points share one terminal workspace」完整需求：執行 `cargo fmt --all -- --check`、Fleety CLI focused/full tests、`cargo clippy -p fleety-cli --all-targets -- -D warnings`，並確認 Chat inline viewport、Settings 返回、standalone Provider editor 與 OAuth 例外路徑無回歸。
