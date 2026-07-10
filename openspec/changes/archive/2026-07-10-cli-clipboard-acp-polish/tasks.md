## 1. Pairing error readability (device-enrollment)

- [x] [P] 1.1 在 `crates/fleety-cli/src/main.rs` 的 `pair` 中，將 `unexpected reply during pair: {other:?}` 的 Debug 傾印改為簡潔可讀訊息（滿足 "Pairing failures surface readable errors"），保留 `Error` 與無 token 分支的既有可讀處理。驗證：新增單元測試斷言非 `Welcome`/`Error` 分支產生的訊息不含 `{`/`:?` 之類 Debug 痕跡且為人類可讀；`cargo test -p fleety-cli`。

## 2. OAuth login port pre-check (codex-oauth)

- [x] [P] 2.1 在 `crates/fleety-cli/src/auth.rs` 的 `login` 中，於 `open_browser` 之前加入固定 loopback port 可用性檢查，被占用時提前回傳可操作錯誤（說明固定 port 死綁與釋放方式），並確保不觸碰既有 token（滿足 "Login fails fast on an unavailable loopback port"）。驗證：新增單元測試先自行 bind 該 port 再呼叫登入前置檢查、斷言回傳可操作錯誤字串；`cargo test -p fleety-cli`。

## 3. Install-script directory selection (self-update)

- [x] [P] 3.1 在 `scripts/install.sh` 以原子寫入探測（在候選目錄建立再刪除暫存檔）取代 `[ -w /usr/local/bin ]`，並確保「不在 PATH」的警告涵蓋回退到 `$HOME/.local/bin` 的情況（滿足 "Sidecar and install paths"）。驗證：`sh -n scripts/install.sh` 通過語法檢查；在非 root shell、未設 `FLEETY_INSTALL_DIR` 下手動走讀腳本邏輯，確認落到 `$HOME/.local/bin` 且印出 PATH 提示。

## 4. ACP session/load response (acp-adapter)

- [x] [P] 4.1 在 `crates/fleety-cli/src/acp.rs` 的 `handle_message` `session/load` 分支，將 `json!({})` 換成依 ACP `LoadSessionResponse`（protocol v1）形狀構造的良好回應（滿足 "session/load returns a conformant ACP response"），並對照 ACP schema/Zed 確認確切欄位。驗證：新增單元測試對 `handle_message` 斷言 load 回應為目標 `LoadSessionResponse` 形狀（非偶然空物件）並在 replay 通知之後；`cargo test -p fleety-cli`；對 Zed 重跑端到端確認被接受。

## 5. Clipboard paste safety and typing (clipboard-paste)

- [x] [P] 5.1 在 `crates/fleety-cli/src/clipboard.rs` 的 `try_attach_as_file` 中，加入最大附件位元組上限常數，超限時不做 base64、改回退為 `ClipboardPaste::Text`（滿足 "Clipboard file attachments are size-bounded"）。驗證：新增單元測試——超過上限的檔案不產生附件（回 None/Text）、界內檔案照常附上；`cargo test -p fleety-cli`。
- [x] 5.2 在 `crates/fleety-cli/src/clipboard.rs` 的 `guess_mime_from_path` 中，將原始碼副檔名映射為可辨識語言的 text MIME（並確認 `try_attach_as_file` 一律保留原始檔名），未知型別維持 `application/octet-stream`（滿足 "Clipboard file attachments carry an identifiable type"）。驗證：新增/擴充單元測試斷言 `.rs`/`.py`/`.go` 映射到可辨識的 text 型別、未知副檔名回 `application/octet-stream`；`cargo test -p fleety-cli`。
