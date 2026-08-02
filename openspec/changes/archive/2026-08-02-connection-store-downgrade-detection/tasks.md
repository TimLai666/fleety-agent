## 1. 儲存格式與載入邊界

- [x] 1.1 實作「Durable connection stores declare a compatible writer contract」與「Store compatibility is an explicit durable contract」：讓 `connections.toml` 寫入並驗證支援的格式版本與 current-writer marker，且所有目前的 durable writer 共用 atomic `0600` 寫入路徑；以 round-trip、權限與所有 writer 的單元測試驗證。
- [x] 1.2 實作「Ambiguous durable state fails closed」：對缺少、格式錯誤或不支援的 marker 回傳可分類的 incompatible-store error，保留原檔並在 credential use、network I/O、profile mutation 前拒絕；以 old-writer rewrite、future version、malformed marker 與無網路/無 credential 使用的測試驗證。
- [x] 1.3 實作「One shared gate covers every durable writer」：讓 CLI server、TUI 設定、Daemon resolver/startup、ACP 與 Doctor 的 durable 讀寫都經過 shared connection-layer gate，transient URL 與 environment target 維持 side-effect free；以 CLI/Daemon smoke tests 和 transient resolution assertions 驗證。

## 2. 明確復原與安全回饋

- [x] 2.1 實作「Incompatible connection stores have an explicit credential-safe recovery path」與「Recovery is explicit and credential-safe」：提供要求更新所有 Fleety binaries、明確 Server URL/profile 與 pairing code 的復原流程，不送出 rejected store 的 token、不猜 learned endpoint、不恢復遺失的 secure proof；以成功重建 marked store 與 rejected credential never reaches transport 的測試驗證。
- [x] 2.2 依照 Implementation Contract 的 Behavior 與 Interface and data shape，讓各 surface 使用穩定的錯誤分類與一致 remediation，並維持既有 per-profile generation 與 authenticated pairing 邊界；以 CLI、TUI、ACP、Doctor、Daemon 的錯誤輸出內容檢查驗證。
- [x] 2.3 依照 Implementation Contract 的 Failures and safety 與 Acceptance criteria，覆蓋 unsupported future store、write failure、atomic replacement、unrelated healthy profiles、transient targets 與檔案保留行為；以 `cargo test -p fleety-tools`、`cargo test -p fleety-cli` 與 `cargo test -p fleety-daemon` 驗證。

## 3. 文件與範圍收斂

- [x] 3.1 實作「Verification covers downgrade evidence and writer boundaries」：更新 CLI、Daemon、TUI 與文件中的 recovery guidance，讓所有 durable writer、credential rejection、permission 與 side-effect-free target 的可觀察契約一致；以文件內容檢查與相關 smoke test 驗證。
- [x] 3.2 依照 Implementation Contract 的 In scope 與 Out of scope，檢查 `crates/fleety-tools/src/connection.rs`、CLI/Daemon call paths、tests、`docs/design-cli-config.md`、`docs/env.md`、`README.md` 都已涵蓋，且沒有改動 transport protocol、mDNS policy 或 legacy binary；以 `spectra analyze connection-store-downgrade-detection --json` 與變更清單人工檢查驗證。
