## Why

`run_command` / `ssh_exec` / `device_exec` 都是一次性「spawn → 等行程結束 → 回傳捕獲輸出」,沒有 stdin、沒有 PTY。剛補的 timeout 讓互動式/卡住的命令不再拖死回合,但**互動需求仍未解**:互動式 TUI、REPL、安裝器提問、`sudo` 密碼、任何需要 TTY 的程式都無法操作——agent 看不到進行中的畫面,也送不了按鍵。本案補上:讓 agent 能開**互動式終端 session**,在 PTY 下跑程式、分回合看輸出並送輸入,且本機、device(經 daemon)、SSH 三種主機都支援。

## What Changes

- 新增一組終端工具(各自一次 ToolResult,沿用既有 tool/RunTool 模型,**協定零變更**):
  - `terminal_open {command, cwd?, ssh_host?, ssh_user?, ssh_port?, ssh_identity?}` → 在 PTY 下開行程,讀到靜默/截止 → `{session_id, output, raw_output, running, exit_code?}`
  - `terminal_input {session_id, data}` → 寫進 PTY,再讀到靜默 → 同形
  - `terminal_read {session_id}` → 行程還在吐就再讀一段
  - `terminal_close {session_id}` → 終止 + 收屍 → `{exit_code}`
- PTY + child 跨呼叫留在**進程全域 session registry**(server 進程供本機/ssh、daemon 進程供 device)。device 透過既有 `device_exec`/`RunTool` dispatch 把每個 `terminal_*` 當普通 on-device 工具一來一回,daemon 進程的 registry 跨呼叫保住 session。
- **一份實作** `PtySession`(portable-pty:Windows ConPTY / unix openpty)。child 是本機 shell/命令或 `ssh -tt host <cmd>`(SSH 只是 argv 變體,不是另一個後端)。
- 互動節奏:open/input 後讀 PTY 直到「安靜一段時間 / 單次截止窗 / child 結束」就回合(可由 env 設定)。
- 回給 agent 的 `output` 去除 ANSI 控制碼(strip-ansi-escapes)易讀,`raw_output` 附原始。
- 安全 + never-crash:`terminal_*` 風險為 Mutate;session 有並發上限、idle-TTL 自動回收、單 session 總時長上限;PTY/child/讀迴圈錯誤一律 errors-as-messages,崩不垮 server/daemon。

## Non-Goals

(本變更會建立 design.md,Non-Goals / 後續寫在 design 的 Goals/Non-Goals 與 Open Questions。)

## Capabilities

### New Capabilities

- `interactive-pty-terminal`: PTY-backed 互動式終端 session——`terminal_open/read/input/close` 工具 + 進程全域 session registry + 讀到靜默的回合制半互動 + ANSI 去碼 + session 上限/TTL 回收;一份 PtySession 實作涵蓋本機 / device(經 daemon)/ SSH(argv 變體);協定不變、agent-core 不碰。

### Modified Capabilities

(none)

## Impact

- Affected specs: interactive-pty-terminal(新)
- Affected code:
  - New:
    - crates/fleety-tools/src/terminal.rs(PtySession + 進程全域 registry + terminal_open/read/input/close 工具 + 讀到靜默 + ANSI 去碼 + session 上限/TTL 回收 + ssh argv 變體)
  - Modified:
    - crates/fleety-tools/Cargo.toml(新依賴 portable-pty、strip-ansi-escapes)
    - crates/fleety-tools/src/lib.rs(mod terminal;匯出 register_terminal)
    - crates/fleety-server/src/tools.rs(伺服器工具註冊掛上 register_terminal)
    - crates/fleety-daemon/src/ondevice.rs(on-device 工具註冊掛上 register_terminal,使 device 經 daemon 也有終端)
    - docs/env.md(終端 session 的 env:讀窗、session 上限、idle-TTL)
