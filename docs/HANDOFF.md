你正在接手 C:\Users\tingzhen\Documents\GitHub\fleety-agent。

# 最終目標

極限打磨 Fleety CLI 的整體使用者體驗，包括：

- 一般 CLI 指令
- 互動式設定流程
- TUI
- 錯誤訊息與狀態顯示
- Server、Daemon、Provider、Model、OAuth、Profile 等設定流程
- 多 Server、多 Daemon、多 Profile 場景
- Windows 實際使用體驗

不要拘泥於現有 CLI 結構或互動形式。請從使用者真正要完成的工作出發，重新思考怎樣的資訊架構、指令命名、互動流程與狀態回饋才最合理。

工作方式必須持續循環：

1. 檢查現況與真實使用流程
2. 改進設計及實作
3. 交給另一個獨立 agent 完整評估
4. 根據評估繼續改進
5. 再交給新的獨立 agent 評估
6. 重複到真正收斂成最佳設計

不要因為主要流程可以跑、測試通過或只剩少數問題，就提早宣布收斂。必須逐項解決 reviewer 發現的實質問題，直到完整獨立評估沒有 Critical、High、Medium 或需要處理的 Low。

GPT reviewer 很慢。每次等待以五分鐘為單位，讓它自然完成，不要提早要求它收斂、摘要或停止。

# 不可違反的設定責任邊界

- CLI 設定只能透過 CLI owner service 修改。
- Daemon 設定只能傳給使用者明確選中的 device/daemon 修改。
- Server、Provider、Model、OAuth 設定只修改目前連線的 Server。
- 如果有多台 Server，目前選中 A，就只能修改 A。
- 永遠不能直接修改設定檔。
- RPC 或 owner service 失敗時，也不能 fallback 成直接修改設定檔。
- 切換 current profile 後，執行中的 fleetyd 必須自動重連，並重新載入新的 Server/Daemon snapshots。
- CLI 使用者可見輸出使用英文。
- 必須處理 Windows 中文亂碼與 UTF-8 問題。

# 工作流程

本專案使用 Spectra。先讀完整 root AGENTS.md 和相關 skill：

- spectra-ingest
- spectra-apply

目前進行中的 change：

redesign-cli-experience

先以目前 worktree 為權威來源，不要假設上一台電腦的局部修改正確或完整。

先執行：

git status --short

git diff -- crates/fleety-daemon/src/main.rs crates/fleety-daemon/tests/fleetyd_smoke.rs crates/fleety-tools/src/connection.rs

git diff -- crates/fleety-server/src/conn.rs crates/fleety-cli/src/provider_service.rs crates/fleety-cli/src/provider_tui.rs

git diff -- crates/fleety-cli/src/auth.rs crates/fleety-protocol/src/lib.rs

git diff -- .agents/skills/spectra-archive/SKILL.md .claude/skills/spectra-archive/SKILL.md .opencode/commands/spectra-archive.md .opencode/skills/spectra-archive/SKILL.md

接著用 spectra-ingest 恢復並核對 redesign-cli-experience，再用 spectra-apply 繼續實作。

# 目前狀態

這個 change 尚未完成：

- Spectra 進度：77/80
- 已完成 5.58：automatic mDNS 只負責探索／顯示；已儲存明確 endpoint 的 current profile 仍會自動連線
- 已完成 5.59：fleetyd 必須先取得 reconnect-control owner 與適用的 service PID owner，失敗時在 poller、dependency、network 前非零退出
- 已完成 5.60：每個 reconnect caller 只接收自己 nonce 的 settlement；舊 terminal result 先持久化成 nonce receipt，後續操作再提交新 nonce
- 5.60 最新獨立複審：No findings；另修正 Windows receipt durability sync 必須使用可寫且不截斷的 handle
- 已完成 5.61：reconnect success 只在 token／identity pin 對凍結 owner 耐久提交、journal 與 success proof 完成後可見；pre-`Welcome`／duplicate `Welcome`／空 minted token／pre-auth presence 全部 fail closed
- 5.61 最新獨立複審：No findings；58 個 daemon unit tests、35 個 fleetyd smoke tests、Clippy、fmt、diff check 與 Spectra gates 全數通過
- 已完成 5.62：Settings dirty profile switch 對每個 owner 都以 Apply 成功加 fresh snapshot 作為 barrier；半成功時保留 fresh revision，refresh 失敗時封鎖後續 owner，所有 owner 完成後才切換
- 5.62 最新獨立複審：No findings；287 個 CLI unit tests、87 個 CLI smoke tests、workspace Clippy、fmt、diff check 與 Spectra gates 全數通過
- 已完成 5.63：daemon reconnect ready/journal contract 已版本化；ready 以程序起始 token 加生命週期 OS lock 防止 PID reuse，publication 以 staged sync、rename、canonical flush 與目錄 sync 保證 crash durability，mixed old/new 或 unknown version 立即回 actionable incompatibility 或走明確 legacy journal 相容路徑
- 5.63 最新獨立複審：No findings；64 個 daemon unit tests、35 個 fleetyd smoke tests、228 個 fleety-tools unit tests、完整 workspace tests、Clippy、fmt、diff check 與 Spectra gates 全數通過
- 已完成 5.64：raw URL、ACP 與 daemon transport override 不再借用 saved credential；只有 resolver 凍結的 named/current profile generation 可寫回 token、TOFU pin 或 auth cleanup，pair/init 的 publication ambiguity 可依同 generation 安全重試，doctor 保持唯讀
- 5.64 最新三個隔離複審：No findings；315 個 CLI unit tests、107 個 CLI smoke tests、69 個 daemon unit tests、41 個 fleetyd smoke tests、237 個 fleety-tools unit tests、329 個 Server unit tests、完整 workspace tests、Clippy、release build、fmt、diff check 與 Spectra gates 全數通過
- 5.65 實作完成、待最終複審：已加入以既有 device token 為 PSK 的 Noise `NNpsk0` 加密握手（`snow`），token 不再上線路，`Welcome`、endpoint 清單與所有後續 frame 都在加密且防竄改的 channel 內；learned endpoint 一律必須通過握手，使用者設定的 endpoint 在 profile 尚未見過安全 Server 前可回退明文，見過後由 `Profile.secure` 永久釘住
- 5.65 已接線的介面：CLI 一次性指令、Chat 初次與重連、ACP turn／cancel、Settings／auth、Provider editor、`doctor` 唯讀探測、fleetyd reconnect，全部共用 `connect_first_healthy`／`open_candidate`；每個 candidate 在單一 deadline 內完成 connect／handshake／`Hello`／`Welcome`／identity
- 5.65 已完成兩輪隔離複審並修正全部 Critical／High／Medium；第三輪複審為交付前的最後一關
- 尚待處理：5.3、5.55
- Windows 交叉編譯仍受本機缺少 MSVC C headers 阻擋於 `ring`；不得誤報成 Windows build 已通過
- Clean review streak：3
- Task 5.3 尚未完成
- 尚未 archive
- 尚未 release
- 不可標記 goal complete
- Worktree 同時包含 5.58～5.64 與先前尚未提交的修改；不得 reset 或覆蓋
- 之前有 workers 在 sandbox reset 後消失，因此必須以實際 diff 判斷修改內容

# 已完成的基準工作

在最新局部修改之前，以下功能已完成並曾經通過完整驗證：

- Protocol config version 5
- Provider API key write-only snapshot
- 使用 key_present 表示金鑰是否存在
- Provider key 支援 Keep、Set、Clear
- CLI 有 --clear-key
- TUI 有 k 清除 key
- Provider catalog 有 auth-disabled gate
- 改善 mDNS profile provenance
- Windows PID 查詢優先使用 tasklist，PowerShell fallback 有 timeout
- 加入 reconnect request/ack 路徑
- Profile switch 使用 live lease
- OAuth terminal output 避免洩漏敏感內容
- Provider/model effects
- Specs、docs、tasks、evidence 曾同步

最近一次完整通過的基準包括：

- cargo test --workspace 全部通過
- CLI smoke tests 75 個通過
- clippy -D warnings 通過
- rustfmt 通過
- git diff --check HEAD 通過
- spectra analyze 為 0 findings
- spectra strict validation 通過
- 三個 binaries 的 release build 通過

但這些結果早於最新局部修改，不能當成目前 worktree 的完成證明。

Windows 平行 build 曾發生 LNK1102 記憶體不足。所有後續 Cargo 驗證先設定：

$env:CARGO_BUILD_JOBS='1'

# 最新 reviewer 的 High 問題

## High 1：Device ConfigApply 繞過 auth-disabled gate

Structured Device ConfigApply 可能繞過 gate，對已連線的 daemon 修改設定。

需要：

- 在 owner dispatch 前建立共用 auth gate
- 加入測試
- 測試必須證明拒絕時完全不會送出 RunTool

## High 2：Reconnect 過早回報成功

fleetyd 可能在新 Server Welcome 和 fingerprint 驗證完成前就回覆 reconnect success。

需要：

- 只有收到新 Server Welcome 且 identity 驗證通過後才能 ACK success
- resolve、connect、auth、identity 任一步失敗都必須 ACK failure
- 所有退出路徑都要 exactly-once settle reconnect ACK
- 不可留下 request 永遠等待或重複回覆

## High 3：Reconnect request 可能遺失

fleetyd 執行長時間 inline tool 時，caller timeout 可能刪除尚未被 daemon 消費的 reconnect request。

需要：

- Request 必須持續保存到 daemon 消費
- Caller timeout 不得刪除 request
- 不得無聲覆蓋既有 request
- 必須測試 timeout、重複 request、daemon 延遲消費與失敗回覆

## High 4：mDNS fingerprint 可偽造

mDNS TXT fingerprint 不是可信的身份證明。現有流程可能在驗證 Server 身份前將 stored token 傳給攻擊者。

安全政策：

- Automatic mDNS discovery 永遠不得附帶 stored token
- Automatic mDNS 只能探索與顯示，不得建立 operational target
- 新的 LAN 候選必須明確選取、提供 pairing code，且收到新 token 後才能存成 current
- 已儲存明確 endpoint 的 current profile 必須照常自動連線並跳過 mDNS
- Credentialed sticky healing 不得只根據 TXT 自動更換 URL
- mDNS 新候選造成的 credentialed endpoint 改變仍必須要求使用者明確 reselect 或 re-pair
- 已驗證 session 內由同一 Server 回報的備援端點可存入該 pinned profile；後續只在 `Welcome` identity 符合既有 pin 時升為 primary
- 不能把 mDNS 宣告值當成已驗證 identity

# 最新 reviewer 的 Medium 問題

## Medium 1：key_present 被丟棄

目前可能透過 filter_map 解析 key_present，之後沒有保留。

需要：

- Strict parse
- 保留在 ProviderSnapshot
- 傳到互動式 UI 和 TUI
- 顯示 key=Set 或 key=Not set
- 不得顯示實際 secret

## Medium 2：Browser launcher 誤判成功

目前可能把 child 成功 spawn 當成瀏覽器已成功開啟。

需要：

- 加 bounded immediate-exit check
- 如果 child 立即非零退出，改走 clipboard fallback
- 不得造成長時間阻塞

## Medium 3：Spectra archive touched-file 順序錯誤

以下四份 instructions 可能在 archive 前刪除 touched file：

- .agents/skills/spectra-archive/SKILL.md
- .claude/skills/spectra-archive/SKILL.md
- .opencode/commands/spectra-archive.md
- .opencode/skills/spectra-archive/SKILL.md

必須改成 spectra archive 成功後才能刪除 touched file。

## Medium 4：Staged index 不同步

這不是產品 bug。

- 未獲得 staging 授權前不要修改 Git index
- 之後若取得授權，只 stage 精確檔案
- 驗證 git diff --cached
- 永遠不要使用 git add -A

# 最新 reviewer 的 Low 問題

## Low 1：OAuth callback 只接受第一個 connection

應在總 deadline 內忽略或拒絕 noise connection，繼續等待有效 OAuth callback。

## Low 2：Protocol history 未記錄 v5

Protocol v5 已實作，但頂層 history docs 可能只記錄到 v4。

## Low 3：target-task548

未追蹤的 target-task548/ 可能包含大量 build artifacts。

- 先確認是否純生成物
- 不要擅自刪除來源不明的檔案
- 不要加入 Git
- 永遠不要 git add -A

# 尚未驗證的 fleetyd 局部修改

上一個 agent 已開始修改 crates/fleety-daemon/src/main.rs，內容可能包括：

- Caller timeout 不再刪除 pending reconnect request
- Timeout 訊息改成 request 仍保留在 queue
- Outer run 加入 pending_reconnect
- 移除過早的 success ACK
- Connect/resolve failure 嘗試回 failure ACK
- serve 接收 pending reconnect
- Welcome 路徑檢查 saved fingerprint 後才回 success ACK
- 加入 expected_target_fingerprint
- 加入 acknowledge_pending_reconnect

這些修改尚未證明可以編譯或通過測試。

已知可能問題：

- Minted token persistence 可能仍發生在 identity/pin 驗證完成前
- 所有 Welcome 前退出路徑是否 exactly-once settle 尚未確認
- 測試尚未完整更新
- 必須逐行 review，不能直接假設正確

# UX 評估要求

除了修 reviewer 列出的問題，也要繼續全面檢查使用流程：

- 第一次安裝與 init
- 無 Server、單 Server、多 Server
- Server 選擇和目前連線狀態
- Profile 新增、切換、刪除、失效
- Daemon online、offline、忙碌、重連中
- OAuth 未登入、登入中、已登入、過期、撤銷
- Provider key 未設定、已設定、清除
- Provider model catalog 成功、空列表、權限不足、網路錯誤
- 手動輸入 model ID
- Server 設定與 Daemon 設定的 owner 是否清楚
- 互動式設定中的返回、取消、儲存、失敗重試
- TUI 中目前焦點、快捷鍵、狀態、錯誤是否一眼能懂
- 非互動模式與 scriptability
- stdout、stderr、exit code
- 無 TTY、SSH、CI、Windows Terminal
- 中文路徑與輸出編碼
- 網路中斷、半成功、重複操作、並行操作
- 敏感資訊是否可能出現在 terminal、log 或 snapshot
- 所有錯誤是否提供下一個可執行動作

不能只做表面文案調整。要追查實際 owner、RPC、狀態同步、時序與安全邏輯。

# 驗證順序

先跑 focused tests：

$env:CARGO_BUILD_JOBS='1'

cargo check --workspace --all-targets --locked

cargo test -p fleety-daemon --bin fleetyd --locked -- --test-threads=1
cargo test -p fleety-daemon --test fleetyd_smoke --locked -- --test-threads=1
cargo test -p fleety-server --bin fleety-server --locked -- --test-threads=1
cargo test -p fleety-cli --bin fleety --locked -- --test-threads=1
cargo test -p fleety-cli --test cli_smoke --locked -- --test-threads=1
cargo test -p fleety-tools --lib --locked -- --test-threads=1

再跑完整 gates：

cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
git diff --check HEAD

spectra analyze redesign-cli-experience
spectra validate redesign-cli-experience --strict

cargo test --workspace --locked -- --test-threads=1
cargo build --release --locked -p fleety-cli -p fleety-server -p fleety-daemon

測試若很慢就繼續等，不要因為暫時沒有輸出而終止。

# Reviewer 規則

每輪實作和驗證後，交給一個沒有參與該輪實作的 agent 評估。

Reviewer 必須：

- 閱讀 AGENTS.md、Spectra artifacts 和實際 diff
- 從終端使用者角度走完整 CLI/TUI 流程
- 同時檢查安全、owner boundary、錯誤處理、race、狀態同步及測試覆蓋
- 自己執行必要驗證
- 依 Critical、High、Medium、Low 回報
- 提供檔案與行號
- 不得只看測試是否通過
- 不得把既有 issue 或前一個 reviewer 的說法直接當真
- 不要被要求提早收斂
- 必須自然完成評估

Reviewer 有實質 findings 時，回到實作階段修正，然後使用新的獨立 reviewer 再評估。

# 完成條件

只有同時符合以下條件，才可以判定目標完成：

- CLI 指令、互動式設定和 TUI 的主要及失敗流程都有一致、直覺的設計
- Owner boundary 全部正確
- 沒有直接設定檔寫入或 fallback
- Profile switch 的 reconnect 與 snapshot reload 有真實測試證明
- OAuth 登入狀態清楚且 catalog 能正確取得模型
- 所有敏感資訊處理安全
- 多 Server、多 Daemon 場景正確
- Windows 與 UTF-8 行為正確
- Spectra tasks、specs、design、evidence 與實作一致
- Focused tests、workspace tests、clippy、fmt、diff check、Spectra validation、release build 全部通過
- 最新獨立 reviewer 沒有尚待處理的實質 findings
- 完成逐項 audit，而不是僅僅沒有發現明顯錯誤

在達成之前持續改進，不要 archive、release 或標記完成。
