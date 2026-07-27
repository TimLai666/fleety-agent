## Context

Chat 現在呼叫 `ratatui::init()` 取得 alt-screen 終端，每一幀重畫整個畫面：對話區、輸入框、狀態列。對話的捲動位置、PageUp／PageDown、滾輪與命中判定都是 Fleety 自己維護的。

行內模式把對話交還給終端機。查證過三件事讓這個改動可行：

- Chat、Settings、Provider 三個面板各自呼叫 `ratatui::init()` 並各自 restore，終端生命週期本來就是分開的。Chat 換成行內終端不需要改動另外兩個。
- `fleety-inline` 的 `emit_to_scrollback` 吃的是帶 ANSI 的字串，而 `fleety-markdown` 已經有 ANSI 輸出路徑（`render_markdown`），不需要額外實作。
- 串流回覆不必等全部收完。`fleety-markdown` 的 `StreamingMarkdownRenderer` 會把已完結的區塊凍結，`frozen_lines_count` 指出哪一段可以安全輸出，未凍結的尾巴留在 viewport。

## Goals / Non-Goals

**Goals:**

- 對話成為真正的終端歷史：可用終端機自己的捲軸捲動、自己的滑鼠選取，程序結束後仍在畫面上。
- 串流中的回覆即時可見，且已完結的部分不必等整輪結束就進入歷史。
- 終端大小改變後歷史不損毀。
- 把 Fleety 重做終端機工作的程式碼刪掉，而不是搬移。

**Non-Goals:**

- 不改 Settings 與 Provider 面板的終端模型。
- 不實作對話的選取、搜尋或捲動。
- 不保留任何形式的滑鼠捕捉。

## Decisions

### Chat owns an inline terminal, Settings and Provider keep their own

Chat 改用 `fleety-inline` 的 `Terminal::with_options` 搭配 `Viewport::Inline`。Settings 與 Provider 維持 `ratatui::init()`。三者本來就各自建立與還原終端，不需要在同一個終端上切換模式。

替代方案是像 Codex 那樣在單一終端上進出 alt-screen。否決原因是 Fleety 的面板是離開 Chat 迴圈後才啟動的獨立流程，共用一個終端物件會把三段生命週期綁在一起，換來的彈性目前沒有人需要。

### The transcript is ANSI text pushed to scrollback, not ratatui lines

完成的訊息以 `render_markdown` 產生 ANSI 字串，交給 `emit_to_scrollback`。Fleety 不再持有對話的顯示表示。

替代方案是自己把 `Vec<Line>` 轉成 ANSI。否決原因是 vendored crate 已經有這條路徑，自己轉會在寬字元與樣式重設上重複踩坑。

### Streaming freezes complete blocks into scrollback and keeps the tail in the viewport

助理回覆進來時餵給 `StreamingMarkdownRenderer`。已凍結的行輸出到 scrollback 且不再重畫；未凍結的尾巴畫在 viewport。整輪結束時把剩餘尾巴一併輸出。

替代方案有兩個。整輪結束才輸出：串流期間 viewport 會長到跟回覆一樣高，等於退回全螢幕。每收到一個 delta 就輸出：程式碼圍籬還沒關閉、表格還沒收尾時輸出會產生錯誤的樣式，而凍結點正是 vendored crate 用來避免這件事的機制。

### The viewport is the unfrozen tail plus composer plus status, with a cap

viewport 高度為未凍結尾巴的行數加上輸入框與狀態列，上限為終端高度的一半。超過上限時尾巴內部捲動，優先顯示最新內容。

上限的存在理由是防止一段很長且遲遲不凍結的內容（例如一個很大的程式碼區塊）把 viewport 撐成全螢幕，那會讓行內模式失去意義。

### Resize re-emits the whole history

保留已輸出的 ANSI 區塊，終端大小改變時以 `resize_purge_rerender` 重送。

替代方案是依賴終端機自己的重排。否決原因是 vendored crate 的註解已經記錄了為什麼不可行：重排在程式收到訊號之前就發生，且各終端行為不一致。

### Mouse capture is removed entirely

不再送出 `EnableMouseCapture`，滑鼠事件不再進入事件管線。輸入框的點擊定位與拖曳選字隨之移除。

理由是行內化之後對話由終端機捲動，捕捉滾輪會直接擋住這件事；剩下需要捕捉的只有輸入框，用一個全域限制（全畫面拖曳要改按 Shift）換取底部數行的便利並不划算。Codex 的做法可佐證：它完全不捕捉滑鼠，只在進入暫時全螢幕時以 DECSET 1007 請終端機把滾輪轉成方向鍵。

## Implementation Contract

**Behavior**

- Chat 啟動後不再清空畫面；先前的終端內容留在原處，Fleety 只在底部佔用 viewport。
- 使用者送出訊息後，該訊息與助理回覆依序成為終端歷史，可用終端機自己的捲軸回看，也可用終端機自己的拖曳選取複製。
- 串流期間已完結的段落陸續進入歷史，尚未完結的尾巴顯示在 viewport 並持續更新。
- 離開 Chat 後，整段對話仍留在終端畫面上。
- 終端寬度改變後，歷史內容不出現錯行或殘影。
- PageUp／PageDown、滾輪與任何滑鼠互動都不再由 Fleety 處理；捲動與選取由終端機負責。

**Interface**

- Chat 以 `fleety_inline::Terminal` 搭配 `Viewport::Inline` 建立終端。
- 完成內容以 `fleety_inline::emit_to_scrollback` 輸出。
- 終端大小改變時呼叫 `fleety_inline::resize_purge_rerender` 與 `fleety_inline::resize_viewport_height`。
- `tui::App` 移除 `scroll_back`、`conversation_area`、`composer_area` 與 `composer_drag`。
- `tui::on_mouse` 與 `tui::Action::CopyToClipboard` 移除。
- `workspace::InputEvent` 與 `WorkspaceInput::recv_event` 移除，`recv` 回到只回傳 `KeyEvent`。

**Failure modes**

- 終端不支援同步輸出時照常繪製，只是可能出現撕裂，不影響正確性。
- `emit_to_scrollback` 失敗時該則訊息不進入歷史，狀態列回報，Chat 繼續運作。
- 非互動環境維持既有行為：`fleety` 裸執行時顯示 help，不建立終端。

**Acceptance criteria**

- Chat 啟動與離開前後，先前的終端內容不被清除。
- 送出一則訊息並收到回覆後，回覆內容出現在 viewport 之上的歷史區，且 viewport 只剩輸入框與狀態列。
- 串流過程中，已完結段落的行數等於 `frozen_lines_count`，且這些行不再重畫。
- 終端寬度改變後重送歷史，內容與改變前一致。
- 全庫不存在 `EnableMouseCapture`，事件管線不傳遞滑鼠事件。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --workspace --all-targets`、`cargo test --workspace -- --test-threads=1` 四者皆通過。

**Scope boundaries**

- In scope：Chat 的終端建立、繪製迴圈、串流輸出、大小改變處理、滑鼠捕捉移除，以及因此失去使用者的程式碼與測試。
- Out of scope：Settings 與 Provider 面板、對話選取與搜尋、`redesign-cli-experience` 尚未完成的項目。

## Risks / Trade-offs

- 移除輸入框的點擊定位與拖曳選字 → 這是今日稍早才加入的功能，使用者已知悉並同意；`tui-mouse-input` 以 REMOVED delta 記錄，不是靜默消失。
- 未凍結尾巴可能很長（大型程式碼區塊）→ viewport 上限為終端高度一半，超過時尾巴內部捲動。
- 已輸出到 scrollback 的內容無法修改 → 只有終端狀態的更新（例如 spinner）留在 viewport，訊息一旦輸出即為定案。
- 既有的整螢幕渲染測試大量失效 → 這些測試斷言的是被移除的對話區，改為斷言輸出到 scrollback 的內容，不是放寬斷言。
- 終端歷史沒有上限，長對話會累積在終端機的 scrollback → 那是終端機的既有行為與使用者的設定，Fleety 不介入。

## Migration Plan

無資料遷移。使用者可感知的變化是對話改由終端機顯示與捲動，且四項捲動或滑鼠行為移除，隨版本說明公告。回退方式是把 Chat 的終端建立改回 `ratatui::init()` 並還原對話區繪製；`fleety-inline` 可留在倉庫中不被引用。

## Open Questions

- 尚未決定 viewport 上限（終端高度一半）是否需要成為可設定值。先以固定值實作，待實際使用後再評估。
