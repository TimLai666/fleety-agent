<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機跨裝置路由（origin 在另一台、經 fleetyd 執行）為環境相依，需手動驗證。 -->

## 1. 解析器（純函式）

- [x] 1.1 [P] 在 crates/fleety-server 新增 workspace 模組：`WorkspaceBinding { root: PathBuf, device: Option<DeviceId> }` 與純函式 `resolve_binding(origin, conn_device, server_host_device, fallback_root)`——絕對 cwd 且 origin=server host → 本機 root；絕對 cwd 且 origin=其他裝置 → 該裝置 remote binding；空/相對/無 origin → fallback root + device None——交付 "Conversation works in the originating directory and device" 與 "Origin cwd is treated as untrusted" 的解析核心（決策「Per-conversation workspace binding resolved once, from the first message」「cwd is untrusted: validate, normalize, keep the guard」「Routing: originating device first, server-host fast path, else fallback」）。驗證:純函式單元測試涵蓋四種情境（含 origin/FLEETY_WORKSPACE/server cwd 的 precedence 以 fallback_root 入參表達）+ 非絕對 cwd 被拒回 fallback;cargo test -p fleety-server 全綠。

## 2. 接線（conn + storage + main）

- [x] 2.1 storage：在對話記錄新增 WorkspaceBinding（root + optional device）欄位，於首則訊息寫入、resume 時讀回；寫入失敗只記錄不中斷——交付 "The workspace binding persists across resume"（決策「Persistence + resume」）。驗證:storage round-trip 單元測試（寫入→讀回一致；缺欄位的舊對話→None）;cargo test -p fleety-server 綠。
- [x] 2.2 conn：首則 UserMessage 用 resolve_binding 解析一次並持久化，之後回合與 resume 重用；把 binding 的 root 與（device 有值時）device-exec 路由接進該對話的 filesystem/command/git 工具建構；後續訊息帶不同 cwd 時忽略（記 log）；origin 缺失/裝置不可達/resume 時裝置離線 → fallback 既有 workspace 並記 log；main.rs 的 workspace_root 降為 fallback 預設——交付 "Conversation works in the originating directory and device"、"Falls back to the server workspace when origin is absent"（決策「Routing: originating device first, server-host fast path, else fallback」「Fallback preserves today's behavior」「Persistence + resume」）。驗證:conn 測試「解析後工具 root 反映 binding」「兩個不同 origin 的對話彼此獨立」「無 origin → fallback（今日行為）」「resume 重用 binding」;cargo build -p fleety-server 綠;實機跨裝置路由標為手動驗證。

## 3. 文件

- [x] 3.1 [P] docs/env.md：記錄工作根 precedence（origin.cwd 在 origin 裝置 → FLEETY_WORKSPACE → server cwd）、cwd 為不可信輸入仍受 FLEETY_FS_SCOPE 與 sensitive-path guard、full_access 不變——交付:文件與行為一致。驗證:內容審查。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機跨裝置路由需手動驗證。
