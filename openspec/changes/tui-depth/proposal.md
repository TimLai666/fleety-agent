## Why

`fleety tui` 的聊天畫面仍陽春:assistant 回覆以純文字整段輸出、沒有 markdown 或程式碼區塊呈現,等待時只有一行靜態 status 沒有動畫或進度感,輸入只能單行、Enter 即送出,附檔只能靠 Ctrl+V,斷線直接退出且離開時未送出的輸入無聲丟失。這些讓實際對話體驗遠低於一般聊天 CLI 的水準(findings #7 #8 #9 #39)。

## What Changes

- Assistant 回覆改為結構化渲染:辨識並排版常見 markdown 子集(標題、清單、粗體、行內碼)與圍欄程式碼區塊(等寬、與內文視覺區隔),取代目前 `message_lines` 的純文字攤平。
- 等待中顯示動畫 spinner:turn 進行中時 spinner 由固定間隔 tick 推進(即使沒有新的 server frame 也會動),並反映已知的 approval / on-device 工具狀態。
- 支援多行輸入:`LineEditor` 升級為可容納換行的緩衝,以明確的換行鍵(Alt+Enter,並提供 Ctrl+J 相容鍵)插入換行,Enter 仍為送出。
- 打字附加檔案:輸入以 `/attach <path>` 指令解析本機路徑為附件,復用 `clipboard::try_attach_as_file` 的檔案偵測與 MIME 猜測,不再只能 Ctrl+V。
- 斷線自動重連:連線中斷時以有上限的指數退避重連,重連後用既有 `Resume`/`Replay` 還原對話,退避用盡才真正結束。
- 離開前確認:Esc 觸發 Quit 時,若尚有未送出的輸入或待送附件,先要求確認,避免內容無聲遺失。

## Non-Goals

- 不做完整 CommonMark 相容渲染(表格、巢狀引用、內嵌圖片、逐語言語法高亮);只涵蓋上述常見子集。是否引入 markdown 渲染函式庫由 design 決定,預設自建最小 renderer 以免拉進重量級相依。
- 不新增 server 端「逐工具執行進度」協定事件;spinner 只反映本地已知的 `turn_in_flight` 與既有 `ApprovalRequested` / `RunTool` 訊號。per-tool 逐步進度屬另一支協定變更。
- 不變更 wire protocol;重連完全復用既有 `Resume`/`Replay`。
- 不觸及 `voice.rs`、`provider_tui.rs`、config TUI(`interactive-config`)與 PTY 工具(`interactive-pty-terminal`)。

## Capabilities

### New Capabilities

- `interactive-chat-tui`: Fleety 聊天 TUI 的呈現與互動契約 — 富文字渲染、等待指示、多行輸入、打字附檔、自動重連、離開前確認。

### Modified Capabilities

(none)

## Impact

- Affected specs: interactive-chat-tui (new)
- Affected code:
  - Modified: crates/fleety-cli/src/tui.rs
  - Modified: crates/fleety-cli/src/input.rs
  - Modified: crates/fleety-cli/src/main.rs
  - Modified: crates/fleety-cli/src/clipboard.rs
  - Modified: crates/fleety-cli/Cargo.toml
  - New: crates/fleety-cli/src/markdown.rs