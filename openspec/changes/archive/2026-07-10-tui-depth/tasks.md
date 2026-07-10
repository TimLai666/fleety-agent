## 1. Markdown 渲染(Rich Assistant Rendering)

- [x] [P] 1.1 新增 crates/fleety-cli/src/markdown.rs,實作純函式 `render(&str) -> Vec<Line<'static>>`,依「Markdown 渲染採最小自建 renderer,不引入重量級函式庫」決策辨識標題/清單/引用/行內 code/bold 與圍欄程式碼區塊;交付:圍欄區塊等寬且與內文區隔、行內樣式套用、未識別語法安全降級。驗證:markdown.rs 內單元測試斷言 fenced-block 行為、inline bold/code span 樣式、未閉合圍欄不 panic。
- [x] 1.2 在 tui.rs 將 assistant/`fleety` 角色訊息的顯示由 `message_lines` 攤平改為呼叫 `markdown::render`,使用者/系統訊息維持逐行;交付 Rich Assistant Rendering 的呈現契約。驗證:tui.rs `TestBackend` 測試斷言含程式碼區塊的回覆在畫面上與內文可區隔、既有 multiline 測試不回歸。

## 2. 等待 spinner(Waiting Indicator With Animated Spinner)

- [x] 2.1 在 App 加入 `spinner_frame` 與推進方法,`input_title()`/status 於 `turn_in_flight` 時插入當前影格;交付 Waiting Indicator With Animated Spinner 的狀態→字串契約。驗證:tui.rs 單元測試斷言連續推進使影格字元改變、idle 時影格不變。
- [x] 2.2 依「Spinner 由固定間隔 tick 驅動」在 main.rs `run_tui` 的 `tokio::select!` 加入 `tokio::time::interval` 分支,tick 時於等待態推進 spinner 並重繪、idle 不重繪。驗證:內容審閱 select! 分支確認 idle 無週期重繪、等待態每 tick 前進;`cargo test -p fleety-cli` 綠燈。

## 3. 多行輸入(Multi-Line Message Composition)

- [x] 3.1 依「多行輸入:LineEditor 升級為多行緩衝,換行鍵用 Alt+Enter/Ctrl+J」讓 input.rs `LineEditor` 允許含 `\n`、新增 `insert_newline()` 與跨行游標,`take()` 保留換行且維持 UTF-8/CJK 邊界安全。驗證:input.rs 單元測試涵蓋插入換行、跨行游標移動、含換行 take、既有 CJK 測試不回歸。（design: 多行輸入:LineEditor 升級為多行緩衝,換行鍵用 Alt+Enter 並相容 Ctrl+J）
- [x] 3.2 在 tui.rs `on_key` 綁定 Alt+Enter 與 Ctrl+J 為插入換行、裸 Enter 維持送出,輸入框渲染支援多列(設高度上限,超限內部捲動保持游標可見);交付 Multi-Line Message Composition 的送出/換行契約。驗證:tui.rs 測試斷言 Alt+Enter/Ctrl+J 不觸發 Send 且插入換行、Enter 送出的 text 含兩行內容。

## 4. 打字附加檔案(Typed File Attachment Command)

- [x] 4.1 依「附加檔案指令:以 /attach 前綴解析路徑,復用 clipboard 的檔案偵測」將 clipboard.rs 的 `try_attach_as_file` 路徑→`WireAttachment` 邏輯提為共用 `pub fn attach_path(&str) -> Option<WireAttachment>`(含 `guess_mime_from_path`),不改既有 Ctrl+V 行為。驗證:clipboard.rs 既有 file/MIME 測試改走共用函式仍通過。
- [x] 4.2 在 tui.rs 送出路徑偵測 `/attach <path>` 指令:存在檔案→`app.attach` 並清空輸入、不送訊息;不存在→保留輸入並於 status 報錯;交付 Typed File Attachment Command 契約。驗證:tui.rs 測試斷言 `/attach` 既存路徑產生 pending attachment 且不回傳 Send、缺失路徑保留輸入且回報錯誤。

## 5. 斷線自動重連(Automatic Reconnection With Backoff)

- [x] 5.1 在 App 追蹤最後 `conversation_id` 與最後事件 `seq`(自 `Assistant`/`Replay` 更新),供重連 `Resume { after_seq }` 使用;交付去重所需狀態。驗證:tui.rs 單元測試斷言收到 Assistant/Replay 後 last-seq 更新、重放同 seq 事件不重覆插入。
- [x] 5.2 依「自動重連:capped 指數退避 + 以 Resume/Replay 還原對話」改 main.rs `run_tui`:`recv_text()` 回傳 `None` 時進入退避重連(狀態列提示、可 Ctrl+C 中止、清 `turn_in_flight`),成功後送 `Resume` 並套用 `Replay`,退避用盡才乾淨退出;交付 Automatic Reconnection With Backoff 契約。驗證:內容審閱重連迴圈的退避上限與去重;手動以中止再重啟 server 觀察 TUI 重連並補齊對話(`cargo run -p fleety-cli -- tui`)。

## 6. 離開前確認(Unsent-Input Quit Confirmation)

- [x] 6.1 依「離開前確認:未送出輸入或待送附件時攔截 Quit」在 App 加入 `confirm_quit` 態並改 tui.rs `on_key` 的 Esc→Quit 路徑:有未送出輸入/附件時首次 Esc 進入確認態(提示文字)、再次 Esc 才 Quit、任何編輯鍵取消確認態;空輸入 Esc 直接 Quit;Ctrl+C 無條件即退。交付 Unsent-Input Quit Confirmation 契約。驗證:tui.rs 測試斷言有輸入時需連按兩次 Esc 才 Quit、編輯鍵取消確認態、Ctrl+C 立即 Quit、turn-in-flight 的 Esc=CancelTurn 與 approval 的 Esc=Deny 優先序不回歸。

## 7. 整合驗證

- [x] 7.1 更新 crates/fleety-cli/Cargo.toml(若 markdown renderer 需新增樣式相關項)並跑 `cargo test -p fleety-cli` 與 `cargo clippy -p fleety-cli`(遵守 workspace 的 unwrap/expect 禁令)全綠。驗證:兩指令輸出無錯誤/警告。