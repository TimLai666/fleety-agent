## Context

Chat 的輸入框原本是 `LineEditor`，一個自寫的單行編輯器，被 Chat、設定面板與 Provider 精靈共用。Chat 需要的是完整的多行編輯體驗；另外兩個只需要單行 filter 與欄位編輯。把選取與還原補進 `LineEditor` 會讓三個使用者共用一個過度複雜的元件。

grok-build 的 `xai-ratatui-textarea` 是自足的：外部相依只有 ratatui、crossterm、textwrap、tui-scrollbar 與兩個 unicode crate，沒有掛任何 grok 內部 crate，附 351 個測試，且釘在 ratatui 0.29，與 Fleety 相同。授權是 Apache-2.0，與 Fleety 相同。上游不接受外部 PR，因此本地修改永遠回不去上游。

Fleety 的對話區是一個扁平的 `Paragraph`，沒有區塊或表格模型。grok 的對話區選取程式碼建立在它自己的區塊與表格幾何上，無法直接對應。

## Goals / Non-Goals

**Goals:**

- Chat 輸入框具備軟換行、還原／重做、readline 刪除與 yank、以詞移動、滑鼠點擊定位與拖曳選取複製。
- vendored 原始碼保持未修改，讓日後重新同步是目錄替換而不是重貼修補。
- 滑鼠回報只影響 Chat；其他路由不需要修改即可維持原行為。
- 終端狀態在正常離開與 panic 兩條路徑都能還原。

**Non-Goals:**

- 不自行實作對話區的文字選取。該行為交給終端機的 Shift 拖曳。
- 不搬移 grok 的行內渲染（`xai-ratatui-inline`）。改變終端模型是另一個決策。
- 不搬移 grok 的 markdown 與語法高亮。那會引入 syntect 與 two-face。
- 不把設定面板與 Provider 精靈改為新輸入框。它們是單行欄位。
- 不修改 vendored `src/`。編輯需求一律改為調整 Fleety 這側的包裝或 Cargo 設定。

## Decisions

### Vendor 為獨立 crate 而非併入 fleety-cli

新增 `crates/fleety-textarea`，`src/` 逐位元組複製，`Cargo.toml` 由 Fleety 自寫。獨立 crate 讓上游的 351 個測試能以 `cargo test -p fleety-textarea` 單獨執行，也讓第三方授權與 Fleety 自有程式碼有清楚邊界。

替代方案是把檔案放進 `crates/fleety-cli/src/`。否決原因是那會讓第三方程式碼與 Fleety 程式碼混在同一個授權與 lint 邊界內，且上游測試無法單獨執行。

### 以 edition 2024 保留原始碼不變，而非改寫為 edition 2021

上游在 9 處使用 let chain，edition 2021 不接受。宣告 `edition = "2024"` 與 `rust-version = "1.85"`，並將 workspace `rust-version` 由 1.80 提高到 1.85。

替代方案是改寫那 9 處以維持 1.80。否決原因是 `src/` 一旦偏離上游，每次重新同步都要重貼修補；而 1.80 這個數字沒有任何 CI 關卡在驗證（CI 使用 stable 工具鏈，倉庫沒有 rust-toolchain.toml）。

### 上游觸發的 lint 以 Cargo 設定豁免，不改原始碼

vendored 測試模組觸發兩條 clippy 規則。在該 crate 的 `[lints.clippy]` 明確列出這兩條為 allow，而不是修改 `src/`。清單是漂移的紀錄，每次重新同步要檢查它有沒有變長。

### 按鍵分工：Fleety 只攔截自己的語意鍵

Fleety 攔截 Ctrl+V、Ctrl+X 與 Enter／Alt+Enter／Esc，其餘全部轉交輸入框的 `input` 方法。Ctrl+X 在輸入框代表剪下、在 Fleety 代表清除已暫存附件，由 Fleety 勝出。

替代方案是在 Fleety 這側重新列出所有編輯鍵。否決原因是兩份鍵位表會隨上游更新而漂移。

### 預填內容時把游標移到結尾

`TextArea::set_text` 保留原游標位置，`LineEditor::set_text` 會移到結尾。所有預填路徑改走一個共用函式，設定內容後把游標設到文字結尾，維持既有行為。

### 事件管線新增滑鼠，但預設出口仍只有鍵盤

終端讀取執行緒改送一個同時涵蓋鍵盤與滑鼠的事件型別。`recv` 過濾掉滑鼠只回傳鍵盤，新增 `recv_event` 回傳兩者。

替代方案是讓 `recv` 直接回傳新型別。否決原因是那會強迫設定面板、Provider 精靈與 config 三個呼叫端處理它們並不需要的事件。

### 命中判定使用上一幀記錄的幾何

繪製時把對話區與輸入框的內框矩形記錄在 `App` 上，滑鼠事件依此判定落點。`Cell` 型別讓繪製函式維持取 `&App`。

替代方案是在事件處理時重算版面。否決原因是版面同時被兩個繪製入口使用，重算會與實際畫面不一致。

### 對話區選取交還終端機

對話區不實作選取，改為在使用者文件說明 Shift 拖曳。終端機的選取已經正確處理折行與寬字元。

### 滑鼠回報在 panic 時也要關閉

`ratatui::init` 的 panic hook 不知道滑鼠回報。以 `Once` 安裝一個前置 hook，在 ratatui 的 hook 之前送出關閉序列。`Once` 是因為這個函式在每次進出設定面板時都會重新執行。

## Implementation Contract

**Behavior**

- Chat 輸入框接受 Ctrl+Z 還原、Ctrl+Y 重做、Ctrl+W 刪除前一個詞、Ctrl+K 刪至行尾、Ctrl+U 刪至行首、Ctrl+Y 貼回最後一次刪除、Alt 或 Ctrl 加左右方向鍵以詞移動。
- 超過框寬的一行折行顯示，輸入框高度隨折行後的行數成長至既有上限，不再水平捲動。
- 在輸入框內按下滑鼠左鍵，游標移到該位置；拖曳形成選取；放開時選取內容送到系統剪貼簿，狀態列顯示複製的字元數；剪貼簿不可用時狀態列明說沒有複製。
- 滾輪在對話區上捲動對話，在輸入框上不捲動對話。
- Ctrl+X 清除已暫存附件且不剪下草稿。
- Enter 送出、Alt+Enter 與 Ctrl+J 換行、Esc 的既有語意不變。
- 離開 Chat 迴圈的兩個出口與 panic 路徑都會關閉滑鼠回報。

**Interface**

- `fleety_textarea::TextArea` 取代 `crate::input::LineEditor` 成為 `tui::App` 的 `input` 欄位型別。
- `tui::on_mouse(&mut App, MouseEvent) -> Action` 為滑鼠入口。
- `tui::Action` 新增 `CopyToClipboard(String)`，由外層迴圈執行實際寫入。
- `tui::prefill(&mut TextArea, &str)` 為所有預填路徑的唯一入口。
- `workspace::InputEvent` 為 `Key` 與 `Mouse` 兩個變體；`WorkspaceInput::recv_event` 回傳兩者，`recv` 只回傳 `Key`。
- `clipboard::write(&str) -> bool` 寫入系統剪貼簿。

**Failure modes**

- 剪貼簿不可用時 `clipboard::write` 回傳 false 並寫入一行 tracing，不中止 TUI。
- `EnableMouseCapture` 失敗時繼續以純鍵盤運作，且不安裝關閉 hook。
- 尚未繪製過任何一幀時滑鼠事件不會命中任何區域，直接忽略。

**Acceptance criteria**

- `cargo test -p fleety-textarea` 通過上游全部測試。
- `crates/fleety-cli/src/tui.rs` 的測試涵蓋：長行折行後首尾同時可見且不在同一列；Ctrl+W 後 Ctrl+Z 還原；Ctrl+X 清除附件且草稿不變；滾輪在對話區改變捲動量、在輸入框不改變；點擊後輸入的字元插入點擊處；拖曳後放開回傳 `Action::CopyToClipboard` 且內容為選取文字。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --workspace --all-targets`、`cargo test --workspace -- --test-threads=1` 四者皆通過。
- `crates/fleety-textarea/src/` 與上游對應目錄比對無差異。

**Scope boundaries**

- In scope：Chat 輸入框、Chat 的滑鼠路由、終端事件管線、剪貼簿寫入、`LineEditor` 因此失去使用者的方法與測試、受 Rust 下限提高影響而新出現的 lint。
- Out of scope：對話區選取、行內渲染、markdown 與語法高亮、設定面板與 Provider 精靈的輸入元件、`redesign-cli-experience` 尚未完成的項目。

## Risks / Trade-offs

- 對話區裸拖曳不再是終端選取 → 在 `docs/tools.md` 明寫 Shift 拖曳，並在 AGENTS.md 記錄這是刻意決定而非未完成項目。
- Rust 下限提高會讓部分使用者無法從原始碼建置 → CI 使用 stable 不受影響；下限本來就沒有關卡驗證，變更記錄在 AGENTS.md。
- Rust 下限提高會解除 clippy 對 MSRV 相關規則的抑制 → 本次已修正三處 `map_or`；AGENTS.md 記錄下次提高下限時會再度發生。
- vendored 原始碼與上游漂移 → `src/` 維持未修改，README 記錄重新同步流程，lint 豁免清單作為漂移紀錄。
- 上游不接受外部 PR → 在此發現的缺陷無法回饋，只能在 Fleety 這側包裝或等待上游修正。
- panic hook 以 `Once` 安裝，若未來有第二個安裝點會互相覆蓋 → 安裝點集中在開啟滑鼠回報之處。

## Migration Plan

無資料或設定遷移。使用者可感知的變化是對話區選取改為 Shift 拖曳，隨版本說明一併公告。回退方式是把 `App` 的 `input` 欄位改回 `LineEditor` 並移除滑鼠回報開關，`fleety-textarea` 可留在倉庫中不被引用。

## Open Questions

- `redesign-cli-experience` 同樣修改 `interactive-chat-tui`，兩份 delta 的封存先後順序需要在封存時確認不衝突。
