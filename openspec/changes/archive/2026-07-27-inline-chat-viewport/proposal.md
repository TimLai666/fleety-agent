## Why

Chat 目前用 alt-screen 佔滿整個終端：離開之後畫面一片空白，對話沒有留下任何痕跡，使用者也不能用終端機自己的捲軸回頭看。Fleety 自己重做了一份捲動（`scroll_back`、PageUp/PageDown、滾輪），只為了補回終端機本來就會做的事。

三個對照組都不是這樣做的。Codex 的終端初始化註解寫明「inline viewport; history stays in normal scrollback」，alt-screen 在它那邊只是可進可出的暫時模式；grok 依賴 `xai-ratatui-inline` 的 `emit_to_scrollback`；Claude Code 的對話同樣留在終端歷史裡。Codex 與 grok 兩個互不相干的團隊，都各自 fork 了 ratatui 的 `Terminal` 才做到這件事，代表這不是設定選項而是需要換掉終端模型。

## What Changes

- Chat 改用行內 viewport：Fleety 只畫底部的輸入框與狀態列，對話交給終端機的 scrollback。
- 一則訊息完成後，以 ANSI 文字送進 scrollback，成為真正的終端歷史：可用終端機自己的捲軸捲動、自己的滑鼠選取，程序結束後仍留在畫面上。
- 串流中的助理回覆畫在 viewport 內，viewport 隨內容長高；回覆完成才送進 scrollback。
- 終端大小改變時重送整段歷史，避免終端機自己的重排破壞已輸出的內容。
- 新增 vendored crate `fleety-inline`（`xai-ratatui-inline` 的逐位元組複本）。
- **BREAKING（使用者可感知）**：以下四項行為被移除，因為它們的職責交還給終端機——
  - 對話區不再由 Fleety 繪製，`scroll_back` 捲動位置隨之移除
  - PageUp／PageDown 捲動對話
  - 滾輪捲動對話
  - 對話區的滑鼠命中判定
- Settings 與 Provider 面板維持 alt-screen 全螢幕，不在本次範圍內。

## Non-Goals

- 不實作對話區的文字選取或搜尋。行內模式之後這本來就是終端機的工作。
- 不改動 Settings 與 Provider 面板的終端模型。它們是全螢幕編輯器，alt-screen 是正確選擇，Codex 對它的全螢幕流程也是這樣處理。
- 不搬移 `xai-ratatui-inline` 之外的任何 grok crate。

## Capabilities

### New Capabilities

- `inline-terminal-viewport`: 行內 viewport 的生命週期、完成訊息如何進入 scrollback、串流中的回覆畫在哪裡、終端大小改變時如何重建歷史，以及哪些顯示職責明確交還給終端機。

### Modified Capabilities

- `tui-mouse-input`: 整個能力移除。Fleety 不再開啟滑鼠回報，捲動與選取交還終端機。

## Impact

檢視過但判定無 spec 層級改動的能力：`interactive-chat-tui`。它的需求談的是 markdown 渲染、spinner、多行組字、附件與離開確認，這些在行內模式下都仍然成立；對話由誰繪製與捲動從未寫進該 spec，屬於實作細節。

- Affected specs: `inline-terminal-viewport`, `tui-mouse-input`
- Affected code:
  - New: `crates/fleety-inline/`（已完成 vendoring）
  - Modified: `crates/fleety-cli/src/main.rs`, `crates/fleety-cli/src/tui.rs`, `crates/fleety-cli/src/workspace.rs`, `crates/fleety-cli/src/markdown.rs`, `crates/fleety-cli/Cargo.toml`, `Cargo.toml`, `docs/tools.md`, `AGENTS.md`
  - Removed: `tui.rs` 的對話區繪製與 `scroll_back` 相關路徑
- Dependencies: 新增 `anstyle-parse`；`fleety-inline` 為新的 workspace 成員。
- Compatibility: 輸入與送出行為不變。使用者可感知的變化是對話改由終端機顯示與捲動，以及四項捲動行為移除。
