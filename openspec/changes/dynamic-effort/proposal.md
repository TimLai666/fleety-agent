## Why

`set_effort` 目前對觸發它的請求幾乎無效。[`crates/fleety-server/src/conn.rs`](crates/fleety-server/src/conn.rs) 在處理一則 top-level 使用者訊息的最開頭讀一次 `session_effort`、把它定型成 `turn_provider`,再傳進 `drive_to_goal`。而 `drive_to_goal` 的參數只有已定型的 `provider`、沒有 `session_effort`,它內部的 goal 連續迴圈(逐次呼叫 `drive_turn`)整段都用同一個 provider。

結果:agent 讀到難任務 → 判斷「很難」→ 呼叫 `set_effort(high)`,但這整個請求(含所有 goal 連續 turn、甚至排隊的後續訊息)都繼續用舊 effort 跑完;高 effort 要等到**下一則 top-level 訊息**才生效,而那可能只是瑣碎追問。等於難任務用低檔跑、調高的效果落在下一件簡單事上,還會賴著不降。

此外,`prompts/` 完全沒有任何指引教 agent 依難度主動調 effort,`set_effort` 工具描述「for your subsequent turns」也對 agent 誤導,讓它以為當前請求會受惠。

## What Changes

- **修正時序(讓 set_effort 對當前請求生效)**:把 `session_effort` 與基礎 provider 一起傳進 `drive_to_goal`,在每次 goal 連續 turn(每次 `drive_turn` 之前)重讀一次 slot、重選 effort provider。mid-request 的 `set_effort` 於下一個連續 turn 立即生效,不再拖到下一則使用者訊息。
- **依難度自動選 effort**:新增一個仿照 `mid-turn-interruption` 的 triage、以便宜模型做的輕量分類器(純解析函式 + graceful 降級)。在 turn 開始前,若自動模式開啟且 agent 未手動釘選 effort,就依使用者訊息難度選 low/medium/high,讓**第一次推理**就在對的檔位。
- **優先序與釋放釘選**:每個 top-level turn 的生效 effort 為「手動釘選 > 自動分類 > 設定預設 > 無」。`set_effort` 增加 `auto` 檔位以清除手動釘選、把控制權交還分類器。
- **設定開關**:新增 `FLEETY_AUTO_EFFORT`(型別化 config registry,預設 on),關閉即回到只靠手動 `set_effort` 的行為。
- **修正描述與補提示**:更正 `set_effort` 工具描述,講清楚「不影響當前這一步、從下一個 turn/連續 turn 起生效並持久」;在 `prompts/protocol.md`(與 `prompts/rules.md` 一句紀律)補上「依難度先設 effort、runtime 也會自動選」的指引。

## Non-Goals (optional)

- **不做逐次內層推理重讀**:重讀粒度定在 goal 連續 turn(`drive_turn`)邊界,不深入 `agent-core` 的內層工具迴圈逐次推理重選 provider。單一 turn 內、set_effort 之後的剩餘內層推理仍不受影響;那個缺口由「難度自動選 effort」在第一次推理前補上。逐次內層重讀列為未來可選強化。
- **不改 subagent 的 effort 模型**:subagent 仍由派生方於 spawn 時決定 effort、無法自調,維持現狀。
- **不改 provider 的 effort 編碼機制**(OpenAI/Gemini scheme、`with_effort`、`effort_field`),沿用既有實作。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `per-task-effort`: 「main agent 動態設定自身 effort」需求改為明訂重讀粒度涵蓋同一請求內的 goal 連續 turn;新增「依任務難度自動選 effort」需求(分類器、優先序、`FLEETY_AUTO_EFFORT`、`auto` 釋放釘選)。

## Impact

- Affected specs: `per-task-effort`(modified)
- Affected code:
  - Modified:
    - crates/fleety-server/src/effort.rs — 難度分類器(assess）、`auto` 清除釘選、優先序輔助
    - crates/fleety-server/src/conn.rs — 將 `session_effort` 與基礎 provider 傳入 `drive_to_goal` 並逐連續 turn 重讀;turn 起始前的難度分類套用
    - crates/fleety-tools/src/config.rs — 新增 `FLEETY_AUTO_EFFORT` 設定
    - prompts/protocol.md — effort 使用指引
    - prompts/rules.md — 一句 effort 紀律
    - docs/env.md — `FLEETY_AUTO_EFFORT` 文件
    - docs/tools.md — 修正 `set_effort` 描述時序
  - New: (none)
  - Removed: (none)
