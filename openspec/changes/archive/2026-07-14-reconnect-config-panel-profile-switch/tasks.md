## 1. 失敗重現與狀態契約

- [x] 1.1 以 TDD 測試重現「面板連到 B、儲存 current=A 後仍持有 B sender」並固定切換前使舊遠端狀態失效契約：切換 intent 產生時 Server/Daemon 的 entries、revision、staged changes 與 availability 全部清除。以 profile_switch_invalidates_old_remote_state 測試先紅後綠驗證。

## 2. Profile 重連交易

- [x] 2.1 實作儲存 current profile 後以明確 profile 建立新連線：connections.toml 儲存成功才關閉舊 sender，重連直接使用已選定 A 的 URL、token 與 fingerprint，不在 async 流程中重新解析全域 current。以 profile_switch_connects_selected_profile_and_closes_old_connection 測試證明 B 不再接收 frame，且連線參數來自 A。
- [x] 2.2 實作新連線分別刷新兩個 owner snapshot 並滿足 Daemon and server regions persist only through their owners：Welcome 後載入 ConfigTarget::Server 與 ConfigTarget::Device(current device id)，各自更新 availability。重連失敗時不恢復 B，Server/Daemon unavailable 而 Connection/CLI 保留。以 profile_switch_reloads_owner_snapshots 與 profile_switch_failure_never_reuses_old_connection 測試驗證成功、daemon 局部失敗及完整失敗路徑。

## 3. 完整驗證

- [x] 3.1 重讀 Implementation Contract，確認切換狀態訊息、staged 隔離、無舊連線 fallback 與 scope boundaries 皆由實作及測試覆蓋。執行 cargo test -p fleety-cli、cargo fmt --all -- --check、cargo clippy -p fleety-cli --all-targets -- -D warnings、spectra analyze reconnect-config-panel-profile-switch 及 spectra validate reconnect-config-panel-profile-switch，所有指令必須成功。
