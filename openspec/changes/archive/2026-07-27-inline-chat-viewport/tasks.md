## 1. 終端模型

- [x] 1.1 實作 `Chat draws into an inline viewport and leaves the screen intact`，依「Chat owns an inline terminal, Settings and Provider keep their own」把 Chat 的終端建立改為 `fleety-inline` 的 inline viewport，Settings 與 Provider 不動。驗證：啟動與離開 Chat 前後，先前的終端輸出仍在畫面上；Settings 與 Provider 的既有測試不需修改即通過。
- [x] 1.2 實作 `Resizing preserves the history`，依「Resize re-emits the whole history」保留已輸出的 ANSI 區塊並在大小改變時重送。驗證：縮窄終端後歷史內容與改變前一致且無殘影，以 headless 重送測試斷言重送內容等於原始區塊序列。

## 2. 對話進入終端歷史

- [x] 2.1 實作 `Completed conversation content becomes terminal history`，依「The transcript is ANSI text pushed to scrollback, not ratatui lines」把完成的訊息以 markdown 的 ANSI 路徑輸出到 scrollback，Fleety 之後不再重畫。驗證：送出一則訊息並收到回覆後，viewport 只剩輸入框與狀態列，且輸出的 ANSI 內容包含該訊息與回覆。
- [x] 2.2 實作 `A streaming reply is visible before it completes`，依「Streaming freezes complete blocks into scrollback and keeps the tail in the viewport」以 `StreamingMarkdownRenderer` 的凍結點決定哪一段可輸出。驗證：串流測試中，已輸出行數等於 `frozen_lines_count`，且未關閉的程式碼圍籬不被輸出。
- [x] 2.3 [P] 實作 `The viewport is bounded`，依「The viewport is the unfrozen tail plus composer plus status, with a cap」限制 viewport 不超過終端高度一半，超過時尾巴內部捲動並顯示最新內容。驗證：以超過半螢幕的未凍結區塊斷言 viewport 高度不超過上限且顯示尾端。

## 3. 移除終端機自己會做的事

- [x] 3.1 實作 `Fleety does not take the mouse`，並移除 `Mouse reporting is scoped to the Chat workspace` 與 `Only Chat receives mouse events`：依「Mouse capture is removed entirely」拿掉滑鼠回報的開關與 panic hook，事件管線回到只傳鍵盤。驗證：全庫搜尋 `EnableMouseCapture` 無結果，且設定面板與 Provider 精靈的測試不需修改即通過。
- [x] 3.2 移除 `The wheel scrolls the transcript only over the transcript`、`Mouse events are resolved against the last rendered geometry` 與 `Transcript text selection belongs to the terminal` 的滑鼠側：拿掉滾輪處理、每幀幾何記錄與命中判定，連同僅涵蓋這些行為的測試。驗證：`cargo clippy --workspace --all-targets -- -D warnings` 不回報 dead code，且 `cargo test --workspace` 全數通過。
- [x] 3.4 移除對話區繪製、`scroll_back` 捲動位置與 PageUp／PageDown 捲動，連同僅涵蓋這些行為的測試。此項與 1.1／2.1 綁在一起：對話區必須先由 scrollback 承接才能拿掉。驗證：`cargo test --workspace` 全數通過，且送出訊息後對話出現在 viewport 之上的歷史區。
- [x] 3.3 移除 `Pointer gestures in the composer place the caret and select text`：拿掉輸入框的滑鼠處理與剪貼簿複製動作，包含 `on_mouse`、`CopyToClipboard` 與對應測試。驗證：`cargo build --workspace --all-targets` 通過且無未使用項目警告。

## 4. 文件與收尾

- [x] 4.1 更新使用者文件與 AGENTS.md：說明對話由終端機顯示與捲動、拖曳選取不需按修飾鍵、Fleety 不再捕捉滑鼠，並移除先前的 Shift 拖曳說明。驗證：檢視 `docs/tools.md` 的 TUI input 段落與 AGENTS.md 的滑鼠段落確已更新且無過期敘述。
- [x] 4.2 全部 CI 關卡通過。驗證：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --workspace --all-targets`、`cargo test --workspace -- --test-threads=1` 四者皆成功。
