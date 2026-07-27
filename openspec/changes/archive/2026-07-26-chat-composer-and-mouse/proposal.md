## Why

Chat 的輸入框是自寫的單行編輯器 `LineEditor`，沒有選取、沒有還原、沒有滑鼠，長訊息只能水平捲動，使用者打到一半看不到開頭。這些缺口不是靠繼續加方法能補平的：選取需要選區模型，還原需要編輯歷史，兩者都會把 `LineEditor` 推向一個完整編輯器，而設定面板與 Provider 精靈只需要它的單行部分。

grok-build（Apache-2.0，與 Fleety 同授權）已經有一個成熟、自足、附 351 個測試的 ratatui 編輯器，且釘在與 Fleety 相同的 ratatui 0.29。與其重寫，不如引入，把自寫的部分縮回它真正被需要的範圍。

## What Changes

- 新增 vendored crate `fleety-textarea`：`xai-ratatui-textarea` 的逐位元組複本，`src/` 不修改，出處、Apache-2.0 §4(b) 修改紀錄與重新同步流程記在該 crate 的 README。
- Chat 的輸入框改用它：軟換行並自動長高、還原／重做、readline 刪除與 yank、以詞為單位移動、滑鼠點擊定位游標、拖曳選取並複製到系統剪貼簿。
- Fleety 只保留自己需要攔截的鍵：Ctrl+V 貼上附件、Ctrl+X 清除已暫存附件、Enter 送出、Alt+Enter 換行、Esc。其餘鍵一律交給輸入框，避免兩份鍵位表各自漂移。
- 開啟滑鼠回報：Chat 期間 `EnableMouseCapture`，離開兩個迴圈出口與 panic hook 都會關閉。滾輪捲動對話區，點擊與拖曳交給輸入框。
- 終端事件管線改送鍵盤與滑鼠兩種事件。`WorkspaceInput::recv` 維持只回傳鍵盤，新增 `recv_event` 供 Chat 使用，其他路由不需修改也不會收到滑鼠事件。
- **BREAKING（使用者可感知）**：對話區的裸拖曳不再是終端機自己的選取，需改按 Shift 拖曳。對話區的選取刻意留給終端機，Fleety 不自行實作。
- 移除 `LineEditor` 因此失去使用者的多行方法與對應測試；設定面板與 Provider 精靈仍使用其單行部分。
- workspace 的 Rust 下限由 1.80 提高到 1.85（vendored crate 為 edition 2024）。

## Capabilities

### New Capabilities

- `tui-mouse-input`: 終端滑鼠回報的開關與生命週期、事件如何送達路由、命中判定依據哪一幀的幾何、滾輪與拖曳各自屬於哪個區域，以及對話區選取交還終端機的界線。

### Modified Capabilities

- `interactive-chat-tui`: 輸入框的編輯契約改為軟換行、還原歷史、readline 鍵位與 kill ring；預填內容時游標落在結尾；Fleety 攔截的鍵與交給輸入框的鍵之間有明確分工。

## Impact

- Affected specs: `tui-mouse-input`, `interactive-chat-tui`
- Affected code:
  - New: `crates/fleety-textarea/Cargo.toml`, `crates/fleety-textarea/README.md`, `crates/fleety-textarea/LICENSE`, `crates/fleety-textarea/src/`
  - Modified: `crates/fleety-cli/src/tui.rs`, `crates/fleety-cli/src/main.rs`, `crates/fleety-cli/src/workspace.rs`, `crates/fleety-cli/src/clipboard.rs`, `crates/fleety-cli/src/input.rs`, `crates/fleety-cli/Cargo.toml`, `Cargo.toml`, `crates/fleety-tools/src/transport.rs`, `crates/fleety-cli/src/auth.rs`, `docs/tools.md`, `AGENTS.md`
  - Removed: none
- Dependencies: 新增 `ratatui-core`、`textwrap`、`tui-scrollbar`；`ratatui` 增加 `unstable-widget-ref` feature；workspace `rust-version` 提高到 1.85。
- Compatibility: 既有鍵位全部保留。滑鼠回報改變終端機的預設選取行為，需要在使用者文件說明 Shift 拖曳。
