## Context

聊天 TUI 的狀態機與渲染(`App` / `on_key` / `render`)集中在 `crates/fleety-cli/src/tui.rs`,單行編輯器在 `input.rs`(`LineEditor`,文件明言 single-line),事件與 WebSocket glue 在 `main.rs::run_tui`,剪貼簿附檔在 `clipboard.rs`。目前:回覆經 `message_lines` 純文字攤平;等待只有靜態 `app.status`;`select!` 迴圈只在有 key 或 frame 時醒來(無週期 tick);`None`(斷線)直接 `should_quit = true`;Esc 於 idle 立即 Quit 丟棄輸入。protocol 已具備 `Resume { conversation_id, after_seq }` 與 `ServerMsg::Replay`,重連無需改 wire。workspace 目前沒有任何 markdown 相依。

## Goals

- 六項可獨立驗證的體驗提升,盡量落在既有 `App`/`LineEditor`/`run_tui` 結構內,狀態轉移仍可用 `TestBackend` 單元測試。
- 不改 wire protocol、不新增 server 事件。
- 渲染與編輯的核心邏輯為純函式,便於測試。

## Non-Goals

- 完整 CommonMark、逐語言語法高亮、server 端逐工具進度事件(見 proposal Non-Goals)。

## Decisions

### Markdown 渲染採最小自建 renderer,不引入重量級函式庫

新增 `crates/fleety-cli/src/markdown.rs`,提供 `render(text: &str) -> Vec<Line<'static>>` 的純函式,辨識行導向的常見子集:ATX 標題(`#`..`######`)、無序/有序清單、`> ` 引用、行內 `` `code` ``、`**bold**`、以及三反引號圍欄程式碼區塊(區塊內不解析行內語法,整段以等寬樣式並加左側標記/縮排與內文區隔)。以 ratatui `Line`/`Span` 的 `Style`(粗體、DIM、可選色)承載樣式,不新增外部相依。開放問題:若後續要更完整的 CommonMark,可換成函式庫(如 pulldown-cmark 產出事件再映射到 `Span`)——此決策點標記為待產品拍板;預設先自建以控制相依與體積。`render` 取代 `tui.rs::message_lines` 中對 assistant/`fleety` 角色訊息的攤平;使用者與系統類訊息維持逐行處理。

### 多行輸入:LineEditor 升級為多行緩衝,換行鍵用 Alt+Enter 並相容 Ctrl+J

`LineEditor` 內部仍為單一 `String`,但允許包含 `\n`;新增 `insert_newline()` 與能跨行移動的游標語意(現有 char-index 游標與 UTF-8 邊界保證不變)。渲染時輸入框改為可顯示多列:高度隨行數在合理上限內成長(超過上限則捲動保持游標可見)。換行鍵綁定 Alt+Enter(`KeyModifiers::ALT` + `KeyCode::Enter`),因多數終端無法可靠區分 Shift+Enter;同時接受 Ctrl+J 作相容鍵。裸 Enter 維持送出。`take()` 回傳含換行的完整文字。失敗模式:終端若把 Alt+Enter 當成 Esc 序列,Ctrl+J 為保底路徑,input 標題列註明可用鍵。

### 附加檔案指令:以 /attach 前綴解析路徑,復用 clipboard 的檔案偵測

送出前於 `on_key` 的 Enter 分支(或送出動作前置)偵測輸入是否為 `/attach <path>` 指令:是則不當一般訊息送出,改把 `<path>` 交給自 `clipboard.rs` 抽出的共用函式(將 `try_attach_as_file` 的路徑→`WireAttachment` 邏輯提為 `pub fn attach_path(path: &str) -> Option<WireAttachment>`,含 `guess_mime_from_path`)。成功則 `app.attach(att)` 並清空輸入、更新 status;路徑不存在則以 status 回報錯誤且不清空輸入。與現有 Ctrl+V 附檔共用同一 `pending_attachments` 佇列與送出契約。

### 自動重連:capped 指數退避 + 以 Resume/Replay 還原對話

`App` 記錄最後看到的 `conversation_id` 與最後事件 `seq`(自 `Assistant`/`Replay` 更新)。`run_tui` 的 `rx.recv_text()` 回傳 `None` 時,不再直接 `should_quit`,改進入重連流程:狀態列顯示「reconnecting…」,以指數退避(如 0.5s→1s→2s…上限如 30s,總嘗試上限如 8 次或封頂後固定間隔重試 N 次)呼叫 `transport::connect` + `hello(...)`,成功後送 `Resume { conversation_id, after_seq }` 讓 server `Replay` 補齊斷線期間事件;`turn_in_flight` 於斷線時清除。退避用盡才 `should_quit = true` 並在狀態列說明。重連迴圈需與 key/tick 分支併存(仍可 Ctrl+C 中止等待)。失敗模式:server 永久不可達→窮盡退避後乾淨退出;重複事件→以 `after_seq` 去重避免重覆插入。

### Spinner 由固定間隔 tick 驅動

`run_tui` 的 `tokio::select!` 新增一個 `tokio::time::interval`(如 120ms)分支:每 tick 於 `turn_in_flight`(或重連中)時推進 spinner 影格索引並觸發重繪。`App` 持有 `spinner_frame: usize`,`input_title()`/status 於等待態插入當前 spinner 影格字元(如 Braille 動畫)。idle 時不動、不重繪浪費。渲染為純狀態→字串,可用單元測試斷言影格隨 tick 遞進。

### 離開前確認:未送出輸入或待送附件時攔截 Quit

`on_key` 的 Esc→Quit(idle)路徑改為:若 `input` 非空或 `pending_attachments` 非空,第一次 Esc 進入 `confirm_quit` 待確認態(status/標題提示「未送出內容 — 再按 Esc 放棄離開,或繼續編輯」),再按一次 Esc 才回傳 `Action::Quit`;任何其他編輯鍵取消待確認態。輸入與附件皆空時 Esc 維持直接 Quit。Ctrl+C 仍無條件立即退出(不受此確認影響)。turn-in-flight 時 Esc 仍為 CancelTurn、approval 待決時 Esc 仍為 Deny,優先序不變。

## Implementation Contract

- `markdown::render(&str) -> Vec<Line<'static>>`:純函式;圍欄程式碼區塊整段等寬且與內文可辨別區隔;行內 `**bold**`/`` `code` `` 於非程式碼行套用對應 `Style`;未識別語法降級為純文字不 panic。
- `LineEditor`:可含 `\n`;`insert_newline()`、多列 `display` 支援;`take()` 保留換行;既有單行測試不回歸。
- 附檔:`/attach <path>` 存在的檔案→暫存附件並清空輸入;不存在→status 報錯、輸入保留;共用 `attach_path`/`guess_mime_from_path`。
- 重連:斷線→退避重連→`Resume`+`Replay` 補事件;`after_seq` 去重;退避用盡→乾淨退出;過程可 Ctrl+C 中止。
- Spinner:`turn_in_flight` 時每 tick 前進一影格並重繪;idle 靜止。
- 離開確認:有未送出內容時需連按兩次 Esc 才 Quit;編輯鍵取消確認態;Ctrl+C 不受影響。
- 範圍邊界:不改 wire protocol、不加 server 事件、不碰 voice/config/PTY。

## Risks

- 終端對 Alt+Enter / Shift+Enter 支援不一致 → 以 Ctrl+J 保底並在標題列標示。
- 自建 markdown renderer 覆蓋不足或誤判邊界(如未閉合圍欄、巢狀強調)→ 一律安全降級為純文字,並以單元測試釘住常見案例。
- 重連期間 `Replay` 與斷線前已顯示事件重覆 → 以 `after_seq` 嚴格去重。
- 多列輸入框改變版面高度,可能擠壓對話區 → 設輸入框高度上限並在超限時內部捲動。
- 開放問題(待拍板):markdown 是否值得引入函式庫、退避的次數/上限具體數值。