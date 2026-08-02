## 1. Nonce 生命週期與控制權

- [x] 1.1 實作「Durable reconnect requests expose one nonce-addressed lifecycle」與「The reconnect nonce owns one observable lifecycle」：讓既有 JSONL journal 可重建 submitted、claimed、in-progress、settled、cancelled、superseded、expired 與 ambiguous 狀態，status 只讀且回傳 nonce、profile、owner、timestamps、replacement 與 retained result；以重啟、重複 status、duplicate nonce 與 conflicting record 測試驗證。
- [x] 1.2 實作「Reconnect cancellation and supersession are owner-safe」與「Cancellation and supersession require the current owner」：加入 owner-safe cancel 與 ordered supersede，禁止 foreign owner、stale owner 或 successor 改寫其他 request，且 durable authenticated success proof 不可被取消；以並發 caller、success-proof、foreign-owner 與 supersession ordering 測試驗證。
- [x] 1.3 實作「Retention and stale recovery are explicit」：為 terminal receipt/proof 設定 retention deadline，讓 active request 不受清理影響，過期資料只在授權下完整 reap；以 active、expired、unrelated record 與 ambiguous ownership 測試驗證。

## 2. Daemon 持續服務與時限

- [x] 2.1 實作「Reconnect housekeeping is bounded and restart-safe」與「Durable housekeeping is bounded and retryable」：讓 journal append、receipt/proof publication、quarantine、directory sync、cleanup 與 shutdown settlement 使用 bounded retry，失敗時保留 evidence、回報 actionable non-success，且不永久持有 reconnect lease 或停止 ordinary service；以 filesystem fault、torn record、restart 與 no-infinite-retry 測試驗證。
- [x] 2.2 實作「Owner-requested reconnects use one caller and sweep budget」與「The caller and sweep share one budget contract」：由單一 documented budget 推導 CLI wait、complete candidate sweep、candidate shares 與 settlement margin，讓 silent candidate 讓出時間給後續 endpoint，ordinary non-reconnect path 保留獨立 budget；以 slow/silent candidate、multi-endpoint sweep、caller timeout 與 ordinary connect 測試驗證。
- [x] 2.3 依照 Implementation Contract 的 Behavior、Interface and data shape、Failures and safety 與 Acceptance criteria，保留 control-version、process-start、owner-generation、authenticated Server identity 與 success-proof 邊界，並讓 malformed/conflicting records fail closed；以既有 receipt-recovery、control-identity、security regression 與完整 Daemon test suite 驗證。

## 3. CLI 與服務生命週期介面

- [x] 3.1 實作「The CLI exposes nonce-addressed reconnect lifecycle operations」：提供 nonce status、owned cancel、owned supersede、control inspection 與 safe stale-control recovery 的穩定操作和 distinct success/refusal classes；以 human-readable、machine-readable、unknown nonce、settled success、foreign owner 與 missing inspection evidence 的 CLI tests 驗證。
- [x] 3.2 實作「Reconnect control ownership supports safe stale recovery」：提供 read-only process identity/process-start/owner generation/nonce/age inspection，只有 dead-owner evidence 或明確確認才能清除，且 live/reused process、successor lock、control-version mismatch 一律保留 artifacts；以 service lifecycle smoke tests 和 read-only assertion 驗證。
- [x] 3.3 實作「Terminal reconnect records follow an explicit retention policy」：讓 receipts 與 success proofs 依文件化期限保留，active/retained records 不被刪除，過期的完整 record 一起 reap 且不碰 unrelated request；以 retention clock、partial record、active journal 與 proof-preservation 測試驗證。
- [x] 3.4 實作「Parallel surfaces stay aligned」：同步 Daemon owner command、CLI notification、service helpers、TUI guidance、smoke tests、docs 與 README 的 nonce、authorization、persistence、output、error guidance；以 cross-surface command review、`cargo test -p fleety-cli`、`cargo test -p fleety-tools`、`cargo test -p fleety-daemon` 與文件檢查驗證。

## 4. 範圍與交付檢查

- [x] 4.1 依照 Implementation Contract 的 In scope 與 Out of scope，確認只改既有 reconnect journal/receipts/proofs、CLI/Daemon/service lifecycle/tests/docs，沒有新增 storage backend、remote protocol、transport exactly-once 或 provider-specific networking；以 `spectra analyze reconnect-control-resilience --json`、變更清單與人工範圍審查驗證。
