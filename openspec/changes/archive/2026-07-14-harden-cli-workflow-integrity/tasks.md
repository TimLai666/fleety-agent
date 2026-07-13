<!--
Each task description MUST state:
- the behavior or contract being delivered (what is observably true when the
  task is complete), and
- the verification target that proves completion (test, CLI invocation,
  analyzer check, manual assertion, or content review).

File paths are supporting context for locating the work, never the task
itself. "Edit file X" is not a valid task — it is missing both behavior and
verification.
-->

## 1. 嚴格解析與錯誤語意

- [x] 1.1 實作「嚴格 parser 區分 help 與 usage error」並滿足 Command parsing is strict and lossless：所有 parser 完整消耗 argv、保留 ask 多字訊息，缺值、無效數字、未知或多餘參數在 I/O 前失敗；以 parser unit tests 與 CLI smoke tests 驗證 help exit 0、usage error exit non-zero。
- [x] 1.2 實作「所有遠端失敗回傳結構化錯誤」並滿足 Remote and protocol failures are process failures：ask、voice、resume、audit、rollback 與 config 僅在收到預期成功 frame 時回成功，malformed JSON 不得顯示空清單；以 mock server smoke tests 驗證 ServerMsg::Error、ok=false、EOF 與 decode failure 都 exit non-zero。

## 2. 目標綁定與交易一致性

- [x] 2.1 實作「Target transaction 不在流程中重新解析」的 pair/init 部分並滿足 Pair and init update only a verified profile：named profile 只更新自身、URL override 不污染 current、init Welcome 成功後才提交；以 temp HOME integration tests 比對成功與失敗前後 connections.toml bytes。
- [x] 2.2 實作「Target transaction 不在流程中重新解析」的 OAuth 部分並滿足 OAuth remains bound to its preflight server：callback 後沿用 preflight Resolved、核對 fingerprint 並設有限 timeout；以 current 中途切換、fingerprint drift 與 callback timeout 測試驗證不會把 credential 送錯 server。
- [x] 2.3 實作「互動編輯器以 dirty state 表達真實持久化狀態」並滿足 Provider editor distinguishes staged and saved state：變更顯示 staged、dirty 離開需 Save/Discard/Cancel，且僅 SaveOutcome::Saved 可啟動 OAuth；以 editor state unit tests 與 conflict/error regression 驗證 staged data 保留且 browser flow 未啟動。

## 3. ACP、服務與更新流程

- [x] 3.1 實作 ACP preserves connection credentials and editor binding：ACP 使用 resolved URL/token、resolver error 不 fallback localhost、unknown verb 失敗、refresh 保留既有 env；以 ACP unit/integration tests 驗證 Hello token、env round-trip 與 unknown verb exit status。
- [x] 3.2 實作「Service 與 update 只接受明確動作」並滿足 Service and update commands have bounded effects：兩個 service binary 的 help/unknown 不進 runtime、daemon up/down 對應 start/stop、必要 child non-zero 使 update 失敗；以 binary smoke tests 和 fake child status 測試驗證作用範圍與 exit status。

## 4. 無副作用查詢與安全持久化

- [x] 4.1 實作「純查詢先於 startup mutation」並滿足 Help and credential writes have no hidden damage 的 help/migration 契約：help/version 不 seed 或 migrate，執行命令的 migration error 不再被忽略；以 temp HOME byte-level smoke tests 驗證查詢零寫入且 migration failure exit non-zero。
- [x] 4.2 實作「外部設定與 credential 使用原子 replace」：Zed settings 與 OAuth token 以同目錄 temp、owner-only 權限及 atomic rename 更新，失敗保留原 bytes；以注入 write/permission/rename failure 的 unit tests 驗證原檔不變且不回報成功。

## 5. 整合驗證

- [x] 5.1 補齊所有 CLI finding 的 regression coverage，並以 `cargo test --workspace -- --test-threads=1` 驗證每項流程契約可重現且已修正。
- [x] 5.2 執行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 與 CLI 手動 smoke matrix，確認格式、lint、help/version 零寫入、provider dirty exit、OAuth timeout 與 update child failure全部符合規格。
