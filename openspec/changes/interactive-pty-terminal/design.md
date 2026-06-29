## Context

工具執行現況:run_command(crates/fleety-tools/src/lib.rs)與 ssh_exec(crates/fleety-server/src/ssh.rs)現在用 tokio process + timeout + kill_on_drop,一次性、無 stdin/PTY。fleety-tools 工具同時註冊在 server(tools.rs build_registry)與 daemon(ondevice.rs build_local_registry),所以放在 fleety-tools 的工具,server 本機與每台 device(經 device_exec/RunTool)都有。協定(fleety-protocol)已有 RunTool dispatch + waiter(server→daemon 一來一回),所有工具皆一次 ToolResult。tokio 已是 fleety-tools 依賴。工作區規則:#![forbid(unsafe_code)]、never-crash errors-as-messages、agent-core 不得依賴任何 fleety crate。

## Goals / Non-Goals

**Goals:**

- agent 能開互動式終端 session,分回合看輸出並送輸入(TUI/REPL/提問/需 TTY 的程式)。
- 本機、device(經 daemon)、SSH 三種主機都支援,且是一份實作。
- 協定零變更、agent-core 不碰、never-crash。

**Non-Goals:**

- 真即時(逐位元組)串流到人類 UI;ANSI 螢幕格點重建(vte);終端 resize/SIGWINCH;co-location 強制;把 session 接到 CLI 人類可視 TUI。皆列後續。

## Decisions

### 回合制半互動 + 持久 session registry(協定零變更)

LLM 回合制與真即時終端衝突,故採「持久 session + 讀到靜默就回合」:`terminal_open/read/input/close` 是普通工具,各回一次 ToolResult;PTY+child 跨呼叫留在**進程全域 registry**(`static REGISTRY: OnceLock<Mutex<HashMap<String, PtySession>>>`)。device session 自動成立——daemon 進程跑同一份 fleety-tools 工具,每個 terminal_* 經既有 RunTool dispatch 一來一回,daemon 進程的 registry 跨呼叫保住 session。理由:沿用既有 tool/RunTool 模型,協定不動;唯一新概念是「進程內存活的 session」。

### 一份 PtySession(portable-pty)+ SSH 為 argv 變體

`PtySession` 用 portable-pty 開 PTY、spawn child、持有 master reader/writer 與 child handle。child 的 argv:本機 = 平台 shell(`cmd /C <command>` 或 `sh -c <command>`,command 空則開互動 shell);SSH = `ssh -tt [opts] host <command>`(沿用 ssh_exec 的 host/user/port/identity 組裝,純函式可測;`-tt` 強制配 PTY)。理由:三後端塌成一份實作,SSH 只是不同 argv;portable-pty 跨平台(ConPTY/openpty)。portable-pty 內部 unsafe 在 crate 內,不違反我們 crate 的 forbid。

### 讀到靜默的回合節奏(純函式判定)

open/input 後從 PTY reader 持續累積位元組,直到任一:**安靜窗**(quiet_gap ≥ quiet_ms,預設 400)、**單次截止窗**(自本回合開始 ≥ max_ms,預設 8000)、或 **child 結束**。其中 quiet_gap = now − max(last_byte, turn_start)——即「距上次輸出」,但若本回合還沒有任何輸出就從回合起始算,給程式回應的時間,避免送出 input 後立刻回空。純函式 `should_stop_reading(quiet_gap, total, child_exited, quiet, max) -> bool`(= exited || total≥max || quiet_gap≥quiet);caller 算 quiet_gap。回合結束後才到的輸出由下個 terminal_read 取得。env:FLEETY_TERMINAL_QUIET_MS、FLEETY_TERMINAL_READ_MAX_MS。理由:務實半互動(非真即時);判定純函式化可測,IO 迴圈薄。

### ANSI 去碼 + 原始並陳

`output` = strip-ansi-escapes 去掉控制碼後的可讀文字(LLM 友善);`raw_output` = 原始位元組 lossy UTF-8(保留供需要時)。理由:TUI 狂吐 escape,去碼後 LLM 才讀得懂;保留 raw 不失真。螢幕格點重建(vte)列後續。

### session 上限 / idle-TTL 回收 / never-crash

registry 有並發上限(FLEETY_TERMINAL_MAX_SESSIONS 預設 8;滿則 terminal_open 回明確錯誤)、每 session 記 last-activity,idle 超過 TTL(FLEETY_TERMINAL_IDLE_TTL_SECS 預設 600)由開新 session 時順手回收(lazy reaper,免背景 task);單 session 總時長上限避免無限掛著。所有 PTY/child/讀寫錯誤、未知 session_id → CoreError 訊息,不 panic、不拖垮進程。理由:互動 session 是長命資源,沒上限會洩漏 PTY/殭屍;lazy 回收最簡單。

### 註冊:server + daemon 都掛上

新增 `pub fn register_terminal(registry: &mut ToolRegistry)`,在 server 的 build_registry(crates/fleety-server/src/tools.rs)與 daemon 的 build_local_registry(crates/fleety-daemon/src/ondevice.rs)各呼叫一次。理由:server 本機 + 每台 device 都要有終端;SSH 變體因需 ssh client,實務上多在 server 跑,但工具本身同一份。

## Implementation Contract

**行為(Behavior):**

- `terminal_open {command:"python3"}` → 開 PTY 跑 python REPL,讀到靜默 → `{session_id, output: 含 ">>>" 提示, raw_output, running:true}`。
- `terminal_input {session_id, data:"print(1+1)\n"}` → 寫入,讀到靜默 → output 含 "2"。
- `terminal_input {data:"exit()\n"}` 或程式自行結束 → 下個讀回合 `running:false, exit_code:0`。
- `terminal_close {session_id}` → 終止仍在跑的 child、移除 session → `{exit_code}`(已結束則回其碼)。
- `terminal_open {command:"uptime", ssh_host:"pi"}` → child = `ssh -tt pi uptime`,其餘同。
- 未知 session_id(input/read/close)→ CoreError 訊息「no such terminal session '<id>'」。
- 超過 MAX_SESSIONS 的 open → 錯誤訊息(含目前數與上限)。

**介面 / 資料形狀:**

- crates/fleety-tools/src/terminal.rs:`struct PtySession { child, writer, reader, started, last_activity, ... }`;`static REGISTRY: OnceLock<Mutex<HashMap<String, PtySession>>>`;工具 TerminalOpen/TerminalRead/TerminalInput/TerminalClose(impl Tool,RiskLevel::Mutate);`pub fn register_terminal(&mut ToolRegistry)`。
- 純函式:`fn should_stop_reading(quiet_gap: Duration, total: Duration, child_exited: bool, quiet: Duration, max: Duration) -> bool`(quiet_gap 由 caller 算 = now − max(last_byte, turn_start));`fn strip_ansi(bytes: &[u8]) -> String`(經 strip-ansi-escapes);`fn ssh_pty_argv(host,user,port,identity,command) -> Vec<String>`;`fn reap_idle(now, ttl, sessions)`(挑出逾時 id)。
- 工具回傳 JSON:open/input/read → `{ session_id, output, raw_output, running, exit_code? }`;close → `{ session_id, exit_code }`。

**失敗模式:**

- PTY 開啟失敗 / spawn 失敗 / 寫入失敗 / reader 錯誤 → CoreError 訊息,session 不建立或標記結束;不 panic。
- 未知 session_id → 錯誤訊息。
- 達 session 上限 → 錯誤訊息,不開。
- child 已死後 input → 回 running:false + 既有 exit_code,不崩。

**驗收標準(Acceptance):**

- 單元測試:should_stop_reading(靜默到/截止到/已結束/都沒 → 對應 true/false);strip_ansi(含 CSI 序列 → 去乾淨);ssh_pty_argv(含 -tt、host/user/port/identity 組裝);reap_idle(挑出逾時 id)。
- 整合測試(跨平台,cfg 選命令):open 一個會回顯的互動程式(unix `cat`、windows `more` 之類,或統一用 python3 若可得;否則 `sh -c`/`cmd` 跑可回顯的)、input 一行、output 含該行回顯、close 回 exit;未知 session_id → Err;達上限(設 MAX=1)第二個 open → Err。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒;真 SSH/真 TUI 互動為手動驗證。

**範圍邊界:**

- In scope:terminal.rs(PtySession + registry + 四工具 + 讀到靜默 + ANSI 去碼 + 上限/TTL + ssh argv)、server/daemon 註冊、docs。
- Out of scope:協定變更、agent-core 變更、真即時串流、vte 螢幕重建、resize、co-location、CLI 人類可視終端。

## Risks / Trade-offs

- [回合制非真即時,極快變動的 TUI 可能在「安靜窗」內仍在動而被切回合] → 可調窗 + agent 可再 terminal_read;務實夠用,真即時列後續。
- [互動 shell 極強(等同 run_command)] → RiskLevel::Mutate 走既有閘;critical 仍靠 run loop 的風險判斷;co-location 列後續。
- [長命 session 洩漏 PTY/殭屍] → 並發上限 + idle-TTL lazy 回收 + 總時長上限。
- [portable-pty 新依賴 + 內部 unsafe] → 跨平台 PTY 自寫會引入我們的 unsafe(違規),用成熟 crate 較安全;forbid 只限我們的程式碼。
- [strip-ansi 失去畫面定位] → raw_output 並陳;vte 重建列後續。

## Migration Plan

- 純加層:新增工具,不動既有 run_command/ssh_exec/協定;不裝新 session 就毫無影響。
- 無資料遷移。回滾:移除 register_terminal 呼叫與 terminal.rs。

## Open Questions

- 真即時(逐位元組)串流到人類可視 UI:需協定串流幀 + turn loop 改造,後續。
- ANSI 螢幕格點重建(vte)取代純去碼:後續。
- 終端 resize / SIGWINCH:後續。
- co-location / 實體場域對互動 shell 的強制:待接入致動裝置時。
