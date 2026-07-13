## Context

CLI 目前以手寫 parser 與多個獨立 async 流程組成。多數函式回 `Result<()>`，但部分 server 錯誤只印字後仍回 `Ok(())`；部分 parser 把 unknown、缺參數與 help 合併；pair、init、OAuth 又在不同時間重新解析 target 或先寫檔。這讓「使用者下的命令、實際作用對象、持久化結果、exit status」無法形成同一個交易。

## Goals / Non-Goals

**Goals:**

- 所有命令嚴格解析並保留完整輸入。
- 失敗必須可由 exit status 與錯誤內容辨識。
- 長流程固定作用於開始時選定的 profile/server。
- 只有持久化成功後才顯示 saved 或啟動 OAuth 等外部副作用。
- help/version、未知 service verb 與 update delegate 不再產生隱性行為。

**Non-Goals:**

- 改用 clap 或新增 CLI framework 依賴。
- 改變成功命令的核心功能或 wire protocol。
- 重做 TUI 視覺風格。

## Decisions

### 嚴格 parser 區分 help 與 usage error

每個 command group 的 parser 必須消耗完整 argv，區分 `Help`、有效命令與 `UsageError`。缺 flag value、無效數字、多餘 positional、未知 flag 都在任何 I/O 前失敗。`ask` 將所有非 flag positional 以空白重組，不再只取第一個字。

### 所有遠端失敗回傳結構化錯誤

共用 helper 將 `ServerMsg::Error`、業務 `ok=false`、EOF 與 JSON decode failure 轉成 `CoreError`。列表 payload 不再 `unwrap_or_default`。命令只有收到符合預期的成功 frame 才回 `Ok(())`。

### Target transaction 不在流程中重新解析

pair、ACP 與 OAuth 在開始時保留完整 `connection::Resolved`。Named profile 的 token/fingerprint 只寫回該 profile；URL override 沒有可持久化 profile 時不污染 current。OAuth callback 後沿用 preflight target，並再次核對 fingerprint。init 將新 profile 暫存在記憶體，Welcome 成功後才提交。

### 互動編輯器以 dirty state 表達真實持久化狀態

Provider editor 每次 mutation 標記 dirty，狀態使用 staged/not saved。離開時進入 Save / Discard / Cancel；只有 `SaveOutcome::Saved` 能產生 OAuth request。Conflict 或 save error 保留 staged state，不執行 OAuth。

### Service 與 update 只接受明確動作

`fleetyd`、`fleety-server` 無參數才進 foreground，`run-service` 才進 service entry；help 回零，unknown 回非零。父 CLI 將 up/down 正規化為 start/stop。`fleety update` 檢查 fleetyd child status 並聚合必要元件失敗。

### 純查詢先於 startup mutation

main 在 config seed 與 legacy migration 前先辨識 help/version。需執行的命令才跑 migration，而且 migration error 會回報。這確保 `--help` 與 `--version` 不修改任何檔案。

### 外部設定與 credential 使用原子 replace

ACP Zed settings 與 OAuth token 先寫同目錄 temp，先套 owner-only 權限，再 rename。Refresh 若未明確提供 server，只改 executable path並保留現有 env。ACP Hello 使用 resolved token，resolver 錯誤不改連 localhost。

## Implementation Contract

**行為:**

- unknown、缺值、多餘參數、無效數字與 remote error 全部 exit non-zero；help exit zero。
- `ask hello world` 送出 `hello world`；任何附件 flag 缺 path 立即失敗。
- malformed conversations/audit/rollback JSON 回 protocol error，不顯示空資料。
- `-s B pair CODE` 只更新 B；URL override 不寫 current A。
- init 失敗不改 connections；成功 Welcome 後才 commit。
- OAuth 在 callback 等待期間切換 current 不影響原交易，server identity 不符則拒絕；callback 有明確 timeout。
- provider dirty state離開需決策，save failure 不啟動 OAuth。
- ACP 沿用 profile token，refresh 不清掉 server env，unknown verb 不進 adapter。
- `fleetyd --help`、`fleety-server --help` 不啟動服務；unknown verb 失敗；up/down 有界地對應 start/stop。
- update 的 fleetyd failure 使整體 non-zero。
- help/version 不觸發 migration。

**介面:**

- group parser 回傳明確的 command/help/usage error。
- ACP bridge 持有 `connection::Resolved` 或等價的 URL + token snapshot。
- OAuth transaction 持有 preflight target、profile source與 fingerprint。
- Provider TUI 增加 dirty 與 exit confirmation state。

**失敗模式:**

- callback timeout、server drift、malformed payload、child non-zero、atomic replace failure都保留原資料並回 actionable error。
- 無法識別的 service/ACP verb 不得落到預設 runtime。

**驗收:**

- 每個 finding 都有對應 unit 或 smoke regression。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace -- --test-threads=1` 全綠。
- 手動驗證 help/version 不改 temp HOME、OAuth timeout、provider dirty exit與 update child failure。

**範圍:**

- In scope：稽核列出的 CLI parsing、exit、target、dirty state、ACP/OAuth、service/update、help migration 與原子寫入。
- Out of scope：設定 owner 路由，已由 `route-config-to-owning-runtime` change 處理。

## Risks / Trade-offs

- [更嚴格 parser 會拒絕過去被忽略的尾端參數] → 錯誤訊息列出正確 usage，避免 typo 被當成功。
- [OAuth timeout 可能讓慢速使用者重試] → 顯示 deadline 與可直接重跑的指引。
- [init 改成後提交] → 失敗 URL 不再自動留作 profile，使用者可先用 `server add` 明確保存不可達目標。
- [dirty confirmation 增加一次按鍵] → 只在有未儲存變更時出現，防止資料遺失與假成功。

## Migration Plan

無資料格式 migration。行為以同版三 binary 出貨。回滾只恢復舊 parser/流程，不需轉換資料。

## Open Questions

無阻斷項。
