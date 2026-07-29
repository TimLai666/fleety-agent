# Fleety CLI UX change handoff

接手目錄：`/Users/timlai/Developer/fleety-agent`

## 最終目標

極限打磨 Fleety CLI 的整體使用者體驗，涵蓋一般指令、互動式設定、
Chat／Settings／Provider TUI、Server／Daemon／Provider／Model／OAuth／Profile
工作流，以及多 Server、多 Daemon、多 Profile、Windows、非 TTY 與失敗復原。

不要因為主要流程可跑或測試通過就提早收斂。每輪修改後都要由沒有參與實作的
獨立 reviewer 檢查 command IA、owner safety、Settings state、TUI heuristics、
accessibility、cross-platform、security 與 regression。修正實質 finding 後重新
覆評，直到連續兩輪沒有 Critical／High，且最後一輪沒有 Medium 或需要處理的
Low。

## 不可違反的責任與安全邊界

- CLI 設定只能透過 CLI owner service 修改。
- Daemon 設定只能傳給使用者明確選中的 device／daemon。
- Server、Provider、Model、OAuth 設定只修改目前連線的 Server。
- RPC 或 owner service 失敗時，不得 fallback 成直接修改設定檔。
- 切換 current profile 後，執行中的 fleetyd 必須自動重連並重載 snapshots。
- 自動 mDNS 只負責探索／顯示。未明確選取並配對的 advertiser 不得取得憑證、
  建立 operational session 或下發控制。
- 已配對 profile 可自動嘗試 Server 在已驗證安全 session 中提供的候選 endpoint；
  每個候選仍須先證明同一 Server 身分，才可取得憑證、接收控制或升為 primary。
- CLI 使用者可見輸出使用英文，並處理 terminal control、UTF-8 與 Windows 中文
  環境。

## Spectra 狀態

- Change：`redesign-cli-experience`
- 進度：92/92
- 已完成：全部 task，包含 5.3、5.55、5.65～5.77
- 未完成：無
- Clean review streak：2
- 尚未 archive、release、commit 或 push 本輪 worktree。
- `.spectra/touched/redesign-cli-experience.json` 必須保留到
  `spectra archive` 成功後。

5.3 已由 5.77 後兩個連續獨立 clean review 完成。5.69 的 evidence、HANDOFF、
完整 Cargo gates、Spectra gates、archive guard 與全文狀態檢查也全部通過。
Change 已完成實作與驗證，但尚未 archive、commit 或 push。

目前沒有任何可追溯來源能證明存在「Settings 九項 Medium」清單。不得補寫、
推測或把舊 reviewer 的其他 findings 改名成那九項。

## 5.65～5.68 已完成行為

### 5.65：已配對 Server 的安全漫遊

- 以既有 device token 作為 Noise `NNpsk0` PSK，先證明 Server 才進入加密控制
  channel。token 不會以明文上線。
- `Welcome`、候選 endpoint 與所有後續 frame 都在加密且具完整性保護的 channel。
- Server 只列舉 non-loopback 介面，不內建或辨識 Tailscale；使用者自行安裝的
  overlay 只會以一般網路介面 endpoint 出現。
- 已驗證 session 提供的候選會歸屬原 profile、去重持久化並 last-successful-first
  重連。learned endpoint 一律不得回退明文。
- CLI one-shot、Chat 初連與重連、ACP turn／cancel、Settings、Provider、OAuth、
  doctor 與 fleetyd 都走同一套候選連線與身分驗證政策。

### 5.66：reconnect receipt crash recovery

- 同 nonce 已存在的 terminal receipt 是權威結果。重啟 recovery 只 reap matching
  journal，不再比較重新建構且可能不同字串的 failure。
- 覆蓋 receipt 已 durable、journal 尚未 reap 就 crash 的 restart regression；
  recovery 後 `ControlGuard::claim` 可再次成功。

### 5.67：reconnect journal append durability

- append writer 開啟前先由 path 取得可信 original length。
- write／sync 失敗使用獨立 writable handle rollback，不依賴 append-only handle
  truncate，並防止 metadata failure 把既有 journal 當成長度 0。
- 覆蓋 metadata、write、sync、rollback-open、rollback-truncate failure，以及
  Windows-compatible handle 行為。

### 5.68：profile generation downgrade detection

- `generation` 已改為 `fleety-profile-v1:<presence-mask>:<opaque nonce>`，綁定
  `endpoints`、`configured_url` 與 `secure` 的 presence state。
- unknown／malformed version 或 versioned generation 與欄位不符時，所有持久化
  profile surface 都在 network／credential use 前 fail closed。
- legacy opaque generation 只在 selected durable owner lease 中升級。健康 profile
  不會因另一個 profile 的已知 v1 mismatch 被阻擋。
- `pair` 不會沿用舊 token／pin。若舊 serializer 已丟失 configured URL，bare pair
  不會猜 learned endpoint；使用者須以明確 URL 重新 enrollment。
- `init <url> --name <profile> --pairing-code <code>` 是窄化的 reenrollment
  transaction，會以 current generation CAS 精確取代該 profile 的 credential、
  pin、learned endpoints 與 secure latch。
- cached durable targets 在每次新 transport 前重新驗證。ownerless raw／environment
  targets 不套用持久化 generation 驗證。

## 最新驗證證據

5.77 最後一次程式修改與 artifacts 同步後已通過：

- `fleety-tools` unit：293 passed
- `fleety-cli` unit：343 passed
- `cli_smoke`：122 passed
- `cargo test --workspace --locked -- --test-threads=1`：全部通過，零失敗
- workspace clippy `-D warnings`：通過
- CLI／Server／Daemon release build：通過
- fmt、diff check、Spectra strict validate、archive instruction guard：通過
- Spectra analyze：零 Critical／Warning，保留兩個既有 Suggestion

Windows native build／runtime 未在這台 macOS 主機實測。Windows handle 契約有
deterministic tests，但不得把它誤報為 Windows-native 驗證。沒有執行 hostile
live LAN advertiser、真實 Tailscale 切網、外部 OAuth browser 或真實多 Server
漫遊；這些仍是 live-environment 限制。

## 最新獨立覆評

`final_convergence_r1` 使用 Sol high 檢查完整 diff，找到一個 Medium 與兩個
actionable Low：candidate advance 未逐次重驗 owner、named pairing 被環境 URL
阻擋、pair 跳過 legacy migration。5.70 已修正並加入 shared sweep、fleetyd 與
CLI smoke regressions；因程式再次修改，clean streak 歸零。

`final_clean_r1` 在 5.70 後找到一個 Medium：Doctor 將 owner 降為 read-only
diagnostic authority 後，共用 per-candidate validator 只檢查 writable owner，
profile 在候選端點之間被替換時仍可能使用舊 token／pin。5.71 已修正並補
read-only owner drift regression；clean streak 維持零。

`post_571_clean_r1b` 在 5.71 後找到一個 Medium 與一個 actionable Low：
Doctor／初始 Settings 的外層 timeout 可能提早取消 roaming sweep，以及
malformed `FLEETY_AGENT_URL` 仍會阻擋明確 named pairing。5.72 已修正並補
Doctor／Settings 多候選與 malformed-env regressions；clean streak 維持零。

`post_572_clean_r1` 在 5.72 後找到兩個 Medium 與一個 actionable Low：
Doctor 在 authenticated `Welcome` 後的 Provider／Daemon snapshot 可無限等待；
fleetyd ordinary SSE downstream 已開啟但 Hello POST 卡住時不會前進；torn
journal 的 read failure 會被忽略。5.73 已以 mandatory candidate deadline、
bounded diagnostic replies 與 fail-closed journal inspection 修正；clean streak
維持零。

`post_573_clean_r1` 在 5.73 後確認一個 Medium：Provider snapshot timeout後，
Doctor 沿用沒有request ID的同一rx做Daemon probe，延遲Server reply可能被誤認
為Daemon成功。該輪完整廣泛審查因執行卡住而中斷，不計clean round。5.74已
改成timeout即關閉診斷session並將後續owner check標為blocked；clean streak維持零。

`post_574_clean_r1` 在 5.74 後找到一個 High、一個 Medium 與一個 actionable
Low：既有 credential 的同 URL `init` 直接送明文 Hello、`connection use` 未在
選取 lease 內綁定 legacy generation，以及 configured URL 遺失時錯誤指向會被
拒絕的 bare `pair`。5.75 已改成 init 先以 durable owner 完成 secure handshake、
選取時原子升級 generation，並直接提供明確 URL `init --pairing-code` 復原指引；
clean streak 維持零。

`post_575_clean_r1` 在 5.75 後找到一個 High 與兩個 Medium：`init` 兩次 load
混用 owner／credential snapshot、latched same-URL init 被不必要拒絕、Settings
persisted B 後仍以不要求 current 的 named target 連 B。第二個隔離 source pass
又確認 `secure=true`、token 已清除時會丟掉 durable owner 與 latch。5.76 已
改成最終單一 snapshot、同 URL owner 保留與 Settings `Target::Current` freeze；
兩輪都有 findings，clean streak 維持零。

`post_576_clean_r1` 在 5.76 後找到一個 High：tokenless secure profile 換到
不同 URL 時仍會丟掉 latch 並走 ownerless cleartext。另一輪找到一個 actionable
Low：測試只驗 pure planner，沒有注入 production 的兩次 load。5.77 已把所有
protected connection state 納入跨 URL pairing gate，並加入 production
interleaving barrier regressions；兩輪都不計 clean，streak 維持零。

`post_577_clean_r1` 在 5.77 後完成完整唯讀覆評，Critical、High、Medium 與
actionable Low 全為零。

`post_577_clean_r2` 從零重查完整 diff、共享連線 producer／consumer、5.77
production interleavings、Settings current-owner freeze 與 reconnect durability，
Critical、High、Medium 與 actionable Low 同樣全為零。兩輪都未修改 worktree；
clean streak 已達 2，5.3 完成。

較早的 `generation_boundary_r2` 隔離檢查 5.66～5.68，結果是零 Critical、High、
Medium、需要處理的 Low。該 clean round 早於 5.70，不再計入目前 streak。

較早的 `continuation_audit` 找到並促成 5.66～5.68：terminal receipt／journal crash
不一致、append rollback durability、舊 binary 丟失 secure state，以及 evidence／
HANDOFF 缺口。該輪有實質 findings，不計入 clean streak。

尚存的 follow-up 已記在 root `AGENTS.md`，包括 reconnect control lifecycle、
reconnect budget、server smoke deadline、舊 binary schema downgrade 可偵測但
無法阻止等。除非新 review 證明它們是目前 change 的 release blocker，不要在
5.69 偽裝成已完成或擅自擴張處理。

## 接手與驗證順序

1. 以 `git status --short`、`git diff HEAD` 與 Spectra artifacts 為權威來源。
2. 跑 `cargo fmt --all -- --check` 與 `git diff --check HEAD`。
3. 跑 focused unit／smoke tests，再跑
   `cargo test --workspace --locked -- --test-threads=1`。
4. 跑 `cargo clippy --workspace --all-targets --locked -- -D warnings`。
5. 跑 `cargo build --release --locked -p fleety-cli -p fleety-server -p fleety-daemon`。
6. 跑 `spectra analyze redesign-cli-experience --json`、
   `spectra validate redesign-cli-experience --strict` 與
   `scripts/check-spectra-archive-instructions.sh`。
7. 由新的獨立 Sol reviewer 檢查完整 diff。不要使用 Tera，也不要要求 reviewer
   提早收斂。
8. 只有連續 clean threshold、5.69 所有 gates 與 artifact 狀態都成立，才完成
   5.3／5.69。Archive、commit、push 與 release 仍是後續獨立動作。

## 完成條件

- CLI、互動式設定與 TUI 的主要及失敗流程一致且可復原。
- Owner boundary、profile generation、credential provenance 與 reconnect 時序正確。
- 多 Server、多 Daemon、多 Profile、OAuth、Provider catalog 與 profile switch
  有 deterministic regression proof。
- Focused tests、workspace tests、clippy、fmt、release build、diff check、Spectra
  analyze／validate、archive guard 全部通過。
- 最新連續兩個獨立 review 沒有 Critical／High，且最後一輪沒有 Medium 或需要
  處理的 Low。
- tasks、specs、design、proposal、evidence 與 HANDOFF 一致。
