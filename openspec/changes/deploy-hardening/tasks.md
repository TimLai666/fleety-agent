## 1. Windows lifecycle verbs pre-flight an elevation check

- [ ] 1.1 在 crates/fleety-tools/src/service.rs 新增無 `unsafe`、無新依賴的 `is_elevated() -> bool`（Windows 以 `net session` 之類指令 exit code 探測；非 Windows 恆真），以及純函式 `elevation_required_message(verb)`，並加單元測試：`elevation_required_message` 對 Install/Start 等變更型動詞含 "administrator"、對 Status 為空字串（比照既有 `admin_hint_only_for_windows_install_uninstall` 測試風格）
- [ ] 1.2 讓 crates/fleety-server/src/service.rs 的 `run(action)` 在 status 以外的動作執行前，對變更型動詞呼叫上述守衛；未 elevated 時回傳 `CoreError::Message`（不呼叫任何 `sc`），由 main.rs 的 `log_action` 印到 stderr 並非零退出。以 `cargo test -p fleety-server service::` 驗證 plan/映射不回歸（實作 requirement: Windows lifecycle verbs pre-flight an elevation check）
- [ ] 1.3 讓 crates/fleety-daemon/src/service.rs 的 install/uninstall/start/stop/restart/enable/disable 在呼叫 `run_verb` 前套用同一守衛，確保 daemon 與 server 行為一致；以 `cargo test -p fleety-daemon` 驗證 spec 建構不回歸
- [ ] 1.4 手動驗證（Windows，非管理員終端）：`fleety-server up` 與 `fleetyd install` 在動手前中止並印出「請以系統管理員身分重新執行」訊息，且 `sc query fleety-server` 顯示服務未被建立（無半套狀態）；`status` 在非管理員下仍正常回報

## 2. Install provisions the insyra sidecar symmetrically

- [ ] 2.1 在 crates/fleety-server/src/main.rs 的 async 生命週期分派中，對 Install 與 Up 動作在 `service::run` 成功後 best-effort 呼叫 `fleety_tools::deps::insyra::ensure_insyra(false).await`，失敗時於 stderr 印出「insyra_exec 在下次 update 成功前不可用」的 console 提示但不改變命令退出碼（比照 crates/fleety-daemon/src/main.rs install 分支；實作 requirement: Install provisions the insyra sidecar symmetrically）
- [ ] 2.2 確認 crates/fleety-server/src/service.rs 的 Install/Up 成功訊息與 2.1 的 sidecar 佈建流程一致（避免重複佈建或訊息矛盾），並以內容審閱確認 fleety-server 與 fleetyd 的 install 佈建路徑對齊
- [ ] 2.3 手動驗證：離線時 `fleety-server up` 仍成功完成且印出 sidecar 佈建失敗提示；有網路時 sidecar 檔案出現在 server 執行檔旁（`sidecar::resolve_insyra` 能解析到）

## 3. Container image runs as a non-root user [P]

- [ ] 3.1 在 Dockerfile 的 runtime stage 建立專屬非 root 使用者（固定 uid，例如 10001），`mkdir -p /data /workspace` 並 `chown` 給該使用者，於 `CMD` 之前加入 `USER`；確認 ddgs 仍安裝在 /usr/local/bin（PATH 上、非 /root/.local/bin），並更新第 26 行附近關於 ddgs 落點的過時註解（實作 requirement: Container image runs as a non-root user）
- [ ] 3.2 手動驗證：`docker build` 後 `docker run --rm <img> id -u` 回傳非 0 的 uid；`docker run -v "$PWD/ws:/workspace" <img>` 讓 server 寫入 /workspace 後，宿主端 `ls -n ws` 顯示檔案 owner 為該 uid 而非 0；容器內 `ddgs --help` 可執行、server 能在 /data 建立 agent 狀態而無權限錯誤

## 4. Verification

- [ ] 4.1 跑 `cargo test -p fleety-tools -p fleety-server -p fleety-daemon` 全綠，確認新增守衛與佈建改動未使既有生命週期/依賴測試回歸
- [ ] 4.2 內容審閱三個 capability 的 delta 是否與實作一致（elevation pre-flight、install-time sidecar 對齊、容器非 root），並確認未擴大到 Non-Goals 排除的範圍（Linux/macOS 權限模型、bind-mount host UID、Compose/k8s）
