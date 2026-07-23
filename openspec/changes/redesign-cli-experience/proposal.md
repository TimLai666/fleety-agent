## Why

Fleety 的功能已涵蓋聊天、連線、Provider、模型、OAuth、遠端設定與服務生命週期，但命令與 TUI 仍依內部模組和設定檔形狀分裂。實測顯示 group help 行為不一致、彙總設定在部分 owner 離線時中途失敗、遠端 Provider 畫面暴露 `providers.toml` 等實作名稱，使用者無法穩定判斷「現在連到哪裡、操作會改哪裡、失敗後是否已套用」。

## What Changes

- 建立單一、宣告式的命令模型，讓頂層與所有子命令一致支援 help、錯字提示、exit code、全域 profile 選擇及機器可讀輸出。
- 以使用者任務重組 Provider、模型、OAuth、連線與診斷命令；舊命令保留相容 alias 與明確棄用提示，不靜默改變腳本語意。
- 裸 `fleety` 在互動終端開啟整合式 TUI，非互動環境顯示 help；`fleety tui` 保留為相容入口。
- 將聊天、Conversation、連線狀態與設定中心納入同一個 TUI shell，固定顯示目前 profile、Server、模型、連線／重連狀態及作用 owner。
- 重做設定中心的資訊架構與狀態模型：依 Connection、CLI、Daemon、Server、Providers & Models 導覽，明示 loading、available、unavailable、dirty、applying、conflict 與 failed 狀態。
- CLI、Daemon、Server 設定仍只由各自 owner 持久化。遠端 owner 不可用時，讀取彙總回傳部分結果與 owner 級錯誤；寫入硬失敗且永不 fallback 寫檔。
- Profile 切換遇到 staged 變更時提供 Apply／Discard／Cancel，不再自動丟棄；連線成功後才重新載入 Server 與 Daemon snapshot。
- 將 mDNS 限縮為不帶 stored credential 的 discovery hint；credentialed endpoint 改變需明確 reselect／re-pair，reconnect request則持久保存到Daemon消費並exactly-once回覆。
- 所有 structured configuration mutation在owner dispatch前共用認證gate，Provider command與TUI保留並顯示不含secret的key presence。
- Provider、模型目錄與 OAuth 狀態整合成同一條旅程，畫面使用「connected Server」與 Provider 名稱，不再以 `providers.toml` 表達操作目的地。
- 新增 `doctor` 與 shell completion，讓連線、版本、Daemon、Server、OAuth／Provider 狀態可被主動診斷，也讓命令可發現。
- 為窄終端、Unicode、鍵盤取消、dirty-state、partial failure、離線與重連建立 headless rendering、parser 與 smoke regression tests。

## Capabilities

### New Capabilities

- `cli-command-surface`: 使用者導向的命令樹、宣告式 help、全域 context、機器輸出、診斷與 completion 契約。
- `terminal-workspace`: 整合聊天、Conversation、目前連線與設定中心的響應式 TUI shell 及一致互動語意。

### Modified Capabilities

- `cli-workflow-integrity`: 將 help、錯字、exit code、相容 alias 與部分成功輸出納入一致契約。
- `interactive-config-panel`: 由四區平面面板改為 owner-aware 設定中心，加入完整 dirty／apply／conflict／profile-switch 狀態。
- `interactive-chat-tui`: 納入共享 shell、連線 context、導覽與一致取消／退出行為。
- `owner-routed-configuration`: 補充讀取彙總的部分可用語意與每次操作顯示實際 owner 的要求。
- `connection-profiles`: canonical 命令改為 connection，並定義 TUI 中安全切換 profile 的互動。
- `service-discovery`: 將 automatic mDNS 的 TXT fingerprint 限縮為排序 hint，任何結果都不得繼承 stored profile credential 或 owner provenance。
- `device-enrollment`: guided／explicit init 遇到 credentialed profile 改址時不得沿用舊 token，必須重新 pairing 後才原子更新。
- `provider-config-surface`: Provider、Model 與 OAuth 形成同一個使用者旅程，隱藏儲存檔實作細節。

## Impact

- Affected specs: `cli-command-surface`, `terminal-workspace`, `cli-workflow-integrity`, `interactive-config-panel`, `interactive-chat-tui`, `owner-routed-configuration`, `connection-profiles`, `service-discovery`, `device-enrollment`, `provider-config-surface`
- Affected code:
  - Modified: `crates/fleety-cli/src/main.rs`, `crates/fleety-cli/src/config.rs`, `crates/fleety-cli/src/config_panel.rs`, `crates/fleety-cli/src/provider_service.rs`, `crates/fleety-cli/src/provider_tui.rs`, `crates/fleety-cli/src/tui.rs`, `crates/fleety-cli/src/server.rs`, `crates/fleety-cli/src/auth.rs`, `crates/fleety-cli/tests/cli_smoke.rs`, `crates/fleety-daemon/src/main.rs`, `crates/fleety-daemon/tests/fleetyd_smoke.rs`, `crates/fleety-protocol/src/lib.rs`, `crates/fleety-server/src/conn.rs`, `crates/fleety-server/src/main.rs`, `crates/fleety-server/tests/server_smoke.rs`, `crates/fleety-tools/src/config.rs`, `crates/fleety-tools/src/connection.rs`, `crates/fleety-tools/src/provider_service.rs`, four generated `spectra-archive` instructions, `README.md`, `docs/design-cli-config.md`, `docs/env.md`, `docs/STATUS.md`
  - New: `crates/fleety-cli/src/commands.rs`, `crates/fleety-cli/src/workspace_tui.rs`
  - Removed: none
- Dependencies: 預計導入集中式 Rust CLI parser 與 completion 支援；最終選型與二進位大小影響記錄於 design。
- Compatibility: 現有腳本命令在本次變更中維持可執行；canonical 名稱變更先透過 alias 與 stderr 棄用提示過渡。
