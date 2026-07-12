## 1. 共用 host-wide 更新（fleety-tools）

- [x] 1.1 依 design「決策三：host-wide sibling 更新抽為共用」與 spec 的 Daemon updates carry the host's sibling binaries 要求，在 crates/fleety-tools/src/update.rs 實作共用的 sibling 更新：對同目錄存在的非自身 fleety binaries 經 manifest_is_templated 守門逐一更新到 latest，fleety-server 更新成功後以其 exe 觸發 bare restart（deferred），缺 {bin} 模板時跳過全部 sibling 並回傳補模板提示。sibling 名單推導與守門抽純函式。先寫測試（tdd）：名單排除自身、缺 {bin} 的跳過訊息。驗證：cargo test -p fleety-tools 全綠。

## 2. daemon（fleetyd）

- [x] 2.1 依 spec 的 Daemon updates carry the host's sibling binaries 要求接線：fleetyd update 動詞在 self-update 與 sidecar 之後呼叫共用 sibling 更新；輪詢 apply tick 依 design「決策四：daemon 輪詢 apply 的重啟順序」在 self_update 之後、request_self_restart 之前帶 sibling。驗證：cargo test -p fleety-daemon 全綠。
- [x] 2.2 依 design「決策二：FLEETY_AUTO_UPDATE 預設 apply」與 MODIFIED spec 的 Release-manifest update polling 要求：auto_apply_enabled 改為未設即 apply、notify 或 0 退回僅通知；既有解析測試同步反轉。驗證：cargo test -p fleety-daemon 全綠。

## 3. server（fleety-server）

- [x] 3.1 依 design「決策一：fleety-server 加 update 動詞」與 spec 的 The server updates itself in place 要求：server 動詞分派新增 update（self_update 經 {bin} 模板 → 換檔則觸發既有 idle-deferred restart；無論換檔與否 ensure_insyra(true) 刷新 sidecar；無 manifest 報既有 actionable 錯誤）。先寫測試：動詞被分派認得（與既有動詞測試同型）。驗證：cargo test -p fleety-server 全綠。
- [x] 3.2 [P] scripts/install-server.sh 尾段在 up 提示旁補 fleety-server update 一行說明。驗證：sh -n 語法通過、內容審閱。

## 4. 文件

- [x] 4.1 [P] docs/env.md 與 README.md：FLEETY_AUTO_UPDATE 預設 apply（notify/0 退回）、fleety-server update、fleetyd update 帶同機 sibling 的說明同步。驗證：內容審閱與 spec 一致。

## 5. 整體驗證

- [x] 5.1 全 workspace 驗證：cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過；CLI 的 fleety update 改用共用 sibling 函式後行為不變（既有測試全綠）。驗證：指令輸出乾淨。
