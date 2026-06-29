<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。實機 Zed session（真實 buffer/terminal/approval）為環境相依，需手動驗證。propose 時先核對 ACP 規格的 fs/terminal 方法名與 capability 欄位。 -->

## 1. ACP↔編輯器 對應與能力 gating（純）

- [x] 1.1 [P] 在 crates/fleety-cli/src/acp.rs 加純對應函式：editor_* 工具呼叫 → ACP 請求形狀（fs/read_text_file、fs/write_text_file、terminal/create+output+wait）＋從 initialize 的 `clientCapabilities` 算出該 advertise 哪些 editor_* 工具（無 terminal→無 editor_run）——交付 "Editor-backed tools execute in the user's editor" 的對應與 gating 核心（決策「Editor tools are a small named set over two ACP primitives」「Capability gating from initialize; tools appear only when usable」「Writes/edits prefer fs (buffer); queries/commands/destructive go via terminal」）。驗證:純單元測試（呼叫→正確 ACP 請求形狀、能力子集→正確工具集）;cargo test -p fleety-cli acp 綠;先核對規格方法名。

## 2. 雙向、持久的 ACP bridge

- [ ] 2.1 在 crates/fleety-cli/src/acp.rs 把 bridge 改成持久＋雙向 JSON-RPC：既答 session/*，又能主動對編輯器發 fs/*、terminal/* 請求；串流回合中收到 server 的 RunTool 就轉成編輯器請求、等回應、回 ToolResult/ToolError；Hello advertise 1.1 算出的 editor_* 工具並回報所在 host device_id——交付 "The adapter is a bidirectional, persistent ACP agent"（決策「The adapter becomes a bidirectional, persistent ACP agent」「Two-level identity」）。驗證:以可注入的編輯器端做 dispatch 測試（routed editor_write_file→fs 寫請求→編輯器回應映射成 ToolResult 帶 surface/saved；不支援能力→無該工具）;cargo test -p fleety-cli acp 綠;實機 Zed 串流中工具往返標手動驗證。

## 3. server 端：per-connection 定址 + 對話綁編輯器 + 路由

- [ ] 3.1 在 crates/fleety-server/src/bridge.rs 為連線指派唯一 per-connection id、讓 hub/pending 能定址特定連線（不只 device_id），使同一 host 多條連線不互撞——交付 "Editor tools target this conversation's editor, identified by host and connection"（決策「Two-level identity: device_id (host) + a per-connection id」）。驗證:測試「兩條共用 device_id 的模擬連線，路由到對話 A 只到 A 的連線」;cargo test -p fleety-server 綠。
- [ ] 3.2 在 crates/fleety-server/src/conn.rs 把對話綁到其服務編輯器連線、註冊該對話的 editor_* 工具（路由到該連線）、並在「該對話有編輯器」時注入「優先用 editor_*＋buffer/磁碟落點＋未存檔提醒」的系統提示；crates/fleety-server/src/tools.rs 放 editor_* 工具規格/路由 proxy——交付 "The agent prefers editor tools and is told how surfaces differ" 與 editor 工具註冊（決策「The agent is told to prefer editor tools and how surfaces differ」「An ACP connection is a per-conversation editor channel, not a device」）。驗證:測試「ACP 對話的系統提示含優先用 editor_* 字句、非 ACP 對話沒有」「editor_* 只在該對話註冊」;cargo build -p fleety-server 綠;實機標手動驗證。

## 4. 文件

- [ ] 4.1 [P] docs/env.md：說明編輯器委派（editor_read_file/write_file/edit=fs buffer、editor_run=terminal/主機；agent 被指示優先用；buffer vs 磁碟與未存檔；ACP 連線回報 host + per-connection 定址；只委派編輯器宣告支援的；Zed 等相容編輯器零改動；跨非編輯器的通用 origin 路由另案）——交付:文件與行為一致。驗證:內容審查。

## 5. 整體驗收

- [ ] 5.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機 Zed session（buffer/terminal/approval）需手動驗證。
