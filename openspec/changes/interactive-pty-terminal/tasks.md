## 1. PTY session 與純函式判定(fleety-tools)

- [x] 1.1 在 crates/fleety-tools/Cargo.toml 加依賴 portable-pty 與 strip-ansi-escapes;新增 crates/fleety-tools/src/terminal.rs 與 lib.rs 的 `mod terminal;`,定義 `PtySession`(portable-pty 開 PTY、spawn child、持有 writer/reader/child handle + started/last_activity)與進程全域 `static REGISTRY: OnceLock<Mutex<HashMap<String, PtySession>>>`,交付 "A persistent PTY session the agent drives turn by turn" 的基礎結構;對應設計「回合制半互動 + 持久 session registry(協定零變更)」與「一份 PtySession(portable-pty)+ SSH 為 argv 變體」。先寫失敗測試:能 new 一個跑回顯命令的 PtySession 並從 registry 取回(cfg 跨平台命令)。
- [x] 1.2 [P] 在 terminal.rs 實作純函式 `should_stop_reading(since_last, total, child_exited, quiet, max) -> bool`、`strip_ansi(&[u8]) -> String`(經 strip-ansi-escapes)、`ssh_pty_argv(host,user,port,identity,command) -> Vec<String>`(含 -tt 與 user@host/-p/-i 組裝),交付 "Each turn reads until the output goes quiet"(判定+ANSI)與 "One PTY implementation covers local, device, and SSH"(ssh argv)的純邏輯;對應設計「讀到靜默的回合節奏(純函式判定)」「ANSI 去碼 + 原始並陳」「一份 PtySession(portable-pty)+ SSH 為 argv 變體」。先寫失敗測試:用 spec example 表逐列驗證 should_stop_reading;strip_ansi 對含 CSI 的位元組去乾淨;ssh_pty_argv 含 -tt、user@host、-p、-i。

## 2. 終端工具(fleety-tools)

- [x] 2.1 在 terminal.rs 實作 `terminal_open`(本機 shell/命令或 ssh argv → 開 PtySession、用 should_stop_reading 讀到靜默、回 session_id/output(去ANSI)/raw_output/running/exit_code?)與 `terminal_read`(對既有 session 再讀一段、同形),讀窗由 env FLEETY_TERMINAL_QUIET_MS(預設400)與 FLEETY_TERMINAL_READ_MAX_MS(預設8000)決定,交付 "A persistent PTY session the agent drives turn by turn" 與 "Each turn reads until the output goes quiet" 的開啟/讀取面;對應設計「回合制半互動 + 持久 session registry(協定零變更)」「讀到靜默的回合節奏(純函式判定)」。先寫失敗測試(cfg 跨平台):open 一個會回顯的程式 → output 含初始輸出、running 反映狀態;terminal_read 對未知 session_id → Err。
- [x] 2.2 在 terminal.rs 實作 `terminal_input`(寫 data 進 PTY、再讀到靜默、同形;child 已死回 running:false + exit_code)與 `terminal_close`(終止仍在跑的 child、移出 registry、回 session_id/exit_code),交付 "A persistent PTY session the agent drives turn by turn" 的輸入/關閉面與其 unknown-session 情境;對應設計「回合制半互動 + 持久 session registry(協定零變更)」。先寫失敗測試(cfg 跨平台):open 然後 input 一行 → output 含該行回顯 → close 回 exit_code;input/close 對未知 session_id → Err。

## 3. session 上限 / 回收 / 註冊

- [x] 3.1 在 terminal.rs 加並發上限(FLEETY_TERMINAL_MAX_SESSIONS 預設8;達上限的 terminal_open 回含目前數與上限的 Err)與 idle-TTL lazy 回收(FLEETY_TERMINAL_IDLE_TTL_SECS 預設600;開新 session 時用純函式 `reap_idle(now, ttl, &sessions)` 挑出逾時 id 並關閉);所有 PTY/spawn/read/write 錯誤走 CoreError、不 panic、單 session 失敗不影響其他,交付 "Sessions are bounded and never crash the host";對應設計「session 上限 / idle-TTL 回收 / never-crash」。先寫失敗測試:reap_idle 挑出逾時 id;MAX=1 時第二個 open → Err。
- [x] 3.2 [P] 新增 `pub fn register_terminal(registry: &mut ToolRegistry)` 註冊四個工具(RiskLevel::Mutate),並在 crates/fleety-server/src/tools.rs 的 build_registry 與 crates/fleety-daemon/src/ondevice.rs 的 build_local_registry 各呼叫一次,使 server 本機與每台 device(經 daemon)都有終端工具,交付 "One PTY implementation covers local, device, and SSH" 的 device 經 daemon 面;對應設計「註冊:server + daemon 都掛上」。驗證:server 與 daemon 的工具註冊都含 terminal_open/read/input/close(編譯期/註冊測試);device session 跨呼叫經 daemon registry 持久(程式碼審查 + 既有 device_exec 路由)。

## 4. 文件

- [x] 4.1 [P] 在 docs/env.md 記錄終端 session 的 env(FLEETY_TERMINAL_QUIET_MS、FLEETY_TERMINAL_READ_MAX_MS、FLEETY_TERMINAL_MAX_SESSIONS、FLEETY_TERMINAL_IDLE_TTL_SECS)與 terminal_* 工具用法(回合制半互動非真即時、SSH 用 ssh_host、output 去 ANSI 與 raw_output 原始、device 經 daemon),交付各 requirement 的文件面。驗證:內容審查涵蓋四個 env、四個工具、ssh_host、ANSI 去碼、回合制半互動說明。
