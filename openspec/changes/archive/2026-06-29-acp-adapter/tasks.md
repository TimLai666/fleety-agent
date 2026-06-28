<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機 editor（如 Zed）啟動 fleety acp 的 live session 為環境相依，需手動驗證。 -->

## 1. JSON-RPC 框架與 ACP 型別

- [x] 1.1 [P] 在 crates/fleety-cli 新增 acp 模組：JSON-RPC 2.0 的 request/response/notification/error 型別與 encode/decode（換行或 Content-Length 框架）——交付 "Fleety runs as an ACP agent over stdio" 的傳輸核心（決策「`fleety acp` is a stdio JSON-RPC front-end that bridges to the server」「Logs to stderr, protocol to stdout」）。驗證:encode/decode round-trip 與 malformed→error 的純函式單元測試;cargo test -p fleety-cli 全綠。

## 2. 方法對映（純函式）

- [x] 2.1 [P] 在 acp 模組實作 ACP↔server 的純對映函式：server AssistantDelta/Assistant → session/update payload、ApprovalRequested → session/request_permission params、Done → stop reason、ACP cwd → OriginContext、session/new → 開對話參數——交付 "ACP methods map to the fleety-server agent"、"Tool approvals surface as ACP permission requests" 的對映核心（決策「Method mapping (ACP ↔ fleety-server)」「Permissions map to `session/request_permission`」「Workspace rooting reuses session-workspace-cwd」）。驗證:每個對映的純函式單元測試（含 cwd→origin、approval→permission、done→stop reason）;cargo test -p fleety-cli 綠。

## 3. 事件迴圈、bridge 與接線

- [x] 3.1 在 acp 模組實作 stdio 事件迴圈與方法分派（initialize/authenticate/session/new/session/load/session/prompt/session/cancel；未知方法→method-not-found）＋ server bridge（重用既有 CLI 連線/探索開 WebSocket、session-id↔conversation-id 映射、把 prompt 轉成 UserMessage 並把 server 串流轉成 session/update、ApprovalRequested 轉 session/request_permission 並回 Approve/Deny、cancel 停止當前回合）；main.rs 加 `acp` 子命令且只在該模式把 log 導向 stderr、stdout 只出 JSON-RPC——交付 "Fleety runs as an ACP agent over stdio"、"ACP methods map to the fleety-server agent"、"Tool approvals surface as ACP permission requests"（決策「`fleety acp` is a stdio JSON-RPC front-end that bridges to the server」「Method mapping (ACP ↔ fleety-server)」「Permissions map to `session/request_permission`」「Logs to stderr, protocol to stdout」）。驗證:分派測試（各方法路由、未知→method-not-found）;以可注入 server transport 的 bridge 測試（一次 prompt → 預期順序的 session/update + 最終 stop reason；ApprovalRequested → request_permission 且回覆對映 Approve/Deny）;cargo build -p fleety-cli 綠;實機 editor live session 標為手動驗證。

## 4. 文件

- [x] 4.1 [P] docs/env.md：說明 `fleety acp`（editor 如何以子行程啟動、server URL/auth 來源、stdout=JSON-RPC/stderr=log、v1 不使用 client fs/terminal、依賴 session-workspace-cwd 做 cwd 根）——交付:文件與行為一致。驗證:內容審查。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機 editor session 需手動驗證。
