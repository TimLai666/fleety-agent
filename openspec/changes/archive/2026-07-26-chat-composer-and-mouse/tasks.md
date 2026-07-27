## 1. Vendor the composer

- [x] 1.1 依「Vendor 為獨立 crate 而非併入 fleety-cli」建立 `crates/fleety-textarea`，`src/` 為上游 `xai-ratatui-textarea` 的逐位元組複本，隨附上游 LICENSE 與記載出處、SOURCE_REV、Apache-2.0 §4(b) 修改紀錄與重新同步流程的 README。驗證：以目錄比對確認 `crates/fleety-textarea/src/` 與上游對應目錄無差異。
- [x] 1.2 依「以 edition 2024 保留原始碼不變，而非改寫為 edition 2021」讓該 crate 在 workspace 內建置並通過上游測試，workspace `rust-version` 由 1.80 提高到 1.85。驗證：`cargo test -p fleety-textarea` 全數通過。
- [x] 1.3 依「上游觸發的 lint 以 Cargo 設定豁免，不改原始碼」讓該 crate 通過專案的 clippy 關卡而不修改 `src/`，豁免清單逐條列出。驗證：`cargo clippy -p fleety-textarea --all-targets -- -D warnings` 無輸出。

## 2. Chat 輸入框

- [x] 2.1 實作 `Composer wraps long lines instead of scrolling sideways`：Chat 的輸入框改由 vendored composer 承擔，長行折行顯示且輸入框隨折行行數成長至既有上限，不再水平捲動。驗證：`tui.rs` 測試 `long_line_wraps_in_the_composer_instead_of_scrolling_sideways` 斷言長行首尾同時可見且不在同一列。
- [x] 2.2 實作 `Composer supports undo, kill and yank`：還原、重做、刪除前一個詞、刪至行尾、刪至行首與 yank 皆可用。驗證：`tui.rs` 測試 `composer_owns_word_kill_and_undo` 斷言刪詞後還原可回復原文。
- [x] 2.3 實作 `Fleety claims only its own chords and delegates the rest`，依「按鍵分工：Fleety 只攔截自己的語意鍵」：Fleety 只攔截 Ctrl+V、Ctrl+X、Enter、Alt+Enter 與 Esc，其餘鍵交給 composer；Ctrl+X 清附件且不剪下草稿。驗證：`tui.rs` 測試 `ctrl_x_clears_attachments_instead_of_cutting_the_draft`。
- [x] 2.4 [P] 實作 `Prefilled composer content places the caret at the end`，依「預填內容時把游標移到結尾」建立單一預填入口，所有程式化填入內容的路徑改走它。驗證：`tui.rs` 測試 `expired_transport_approval_cannot_be_committed_and_preserves_composer` 斷言預填後輸入的字元出現在結尾。
- [x] 2.5 [P] 實作 `Composer state survives independently of its implementation`：草稿與游標在路由切換與重連後保持不變，游標以文字內位置表達而非螢幕座標。驗證：`workspace.rs` 測試 `workspace_session_preserves_multiline_draft_cursor_and_attachment_across_routes` 與 `main.rs` 的重連測試以位元組位移斷言。
- [x] 2.6 移除 `LineEditor` 因 Chat 改用 composer 而失去使用者的多行方法與僅涵蓋這些方法的測試，設定面板與 Provider 精靈維持可用。驗證：`cargo clippy --workspace --all-targets -- -D warnings` 不再回報 dead code，且 `cargo test --workspace` 全數通過。

## 3. 滑鼠輸入

- [x] 3.1 實作 `Only Chat receives mouse events`，依「事件管線新增滑鼠，但預設出口仍只有鍵盤」讓終端讀取執行緒同時傳遞鍵盤與滑鼠事件，預設讀取仍只回傳鍵盤，Chat 透過另一個讀取取得兩者。驗證：`workspace.rs` 測試 `handoff_rejects_an_old_epoch_key_that_arrives_after_the_boundary` 仍通過，且設定面板與 Provider 精靈的測試不需改動。
- [x] 3.2 實作 `Mouse reporting is scoped to the Chat workspace`，依「滑鼠回報在 panic 時也要關閉」在 Chat 迴圈開啟滑鼠回報，於兩個離開分支與 panic 路徑關閉；開啟失敗時 Chat 以純鍵盤運作。驗證：檢視 Chat 迴圈的兩個離開分支與 panic hook 皆送出關閉序列，且開啟失敗時不安裝 hook。
- [x] 3.3 實作 `Mouse events are resolved against the last rendered geometry`，依「命中判定使用上一幀記錄的幾何」在繪製時記錄對話區與輸入框的內框，尚未繪製時忽略事件。驗證：`tui.rs` 的滑鼠測試先繪製一幀再取用記錄的區域座標。
- [x] 3.4 [P] 實作 `The wheel scrolls the transcript only over the transcript`：滾輪在對話區捲動對話，在輸入框不捲動對話。驗證：`tui.rs` 測試 `wheel_over_the_transcript_scrolls_it` 與 `wheel_below_the_transcript_does_not_scroll_it`。
- [x] 3.5 [P] 實作 `Pointer gestures in the composer place the caret and select text`：按下移動游標、拖曳形成選取、放開時送到系統剪貼簿並回報字元數，剪貼簿不可用時明說沒有複製。驗證：`tui.rs` 測試 `clicking_the_composer_moves_the_caret_into_the_draft` 與 `dragging_the_composer_selects_and_asks_the_loop_to_copy`。

## 4. 文件與收尾

- [x] 4.1 實作 `Transcript text selection belongs to the terminal`，依「對話區選取交還終端機」在使用者文件說明 Chat 的輸入按鍵表與 Shift 拖曳選取對話區，並在 AGENTS.md 記錄這是刻意決定、vendored crate 不進自動更新流程、以及只有 Chat 讀取滑鼠事件。驗證：檢視 `docs/tools.md` 的 TUI input 段落與 AGENTS.md 的 Vendored Rust source 段落確有上述內容。
- [x] 4.2 修正因 Rust 下限提高而新出現的 clippy 規則命中處，並在 AGENTS.md 記錄下次提高下限時會再度發生。驗證：`cargo clippy --workspace --all-targets -- -D warnings` 無輸出。
- [x] 4.3 全部 CI 關卡通過。驗證：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --workspace --all-targets`、`cargo test --workspace -- --test-threads=1` 四者皆成功。
