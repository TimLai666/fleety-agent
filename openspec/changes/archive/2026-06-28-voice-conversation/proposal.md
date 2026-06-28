## Why

語音對話是 spec-v0 §11 規劃的 M7 能力（post-v0），但目前協定層 0 實作：`fleety-protocol` 沒有 speech 欄位、沒有 voice mode 旗標，`fleety-cli` 只有 audio 作為輸入附件，沒有 STT/TTS。spec-v0 §12 宣稱「協定欄位 M0–M2 就先留」與現況不符。要讓使用者能用講的跟 agent 對話、agent 在達成目標或必問時用口語回覆，需要把這條通道實際做出來。剛完成的 goal-completion 已在 conn 提供 `emit_terminal`（只有終止回合 emit 使用者回覆），語音正好掛在這個機制上——口語只在終止回合產出，中間續做回合自然不出聲。

## What Changes

- **協定（fleety-protocol）**：`ClientMsg::UserMessage` 新增 `voice: bool` 旗標（per-message，預設 false）；`ServerMsg::Assistant` 新增 `speech: Option<String>`（口語版，僅終止回合且 voice on 時帶值）。兩者皆用 serde `default`/`skip_serializing_if`，向後相容，`PROTOCOL_VERSION` 不需 bump。
- **agent-core 雙通道輸出**：voice on 的回合，系統 prompt 要求模型在最終回覆附帶一段可解析的口語區塊；`run_turn`/`run_turn_streaming` 的 `TurnOutcome` 新增 `speech: Option<String>`，由 core 從模型最終訊息解析出 display 與 speech 兩段。voice off 時完全不要求、不解析、不產 speech（不花 token）。非 voice 與既有單通道呼叫點（subagent、recovery）行為不變。
- **server 整合（fleety-server/conn）**：turn driver 帶 voice 旗標；voice on + 終止回合時，從 `TurnOutcome` 取 speech 一併放進 `ServerMsg::Assistant.speech` emit；中間續做回合不產口語；`ask_user` 的問題同樣有口語版。
- **終端 STT/TTS（fleety-cli，OS 原生）**：STT 把語音輸入轉文字並送出 `voice: true` 的 `UserMessage`；TTS 把收到的 `speech` 欄位用 OS 原生引擎（Windows SAPI、macOS `say`、Linux speech-dispatcher）唸出。引擎缺失或失敗時優雅退回純文字（never-crash）。
- **文件同步**：`prompts/protocol.md` 的 Output Channels 段從「行為已描述、runtime 未實作」更新為實作後狀態；`docs/tools.md`、`docs/env.md`（新增 voice 相關 env，如 TTS 開關/語音名）同步；修正 `docs/spec-v0.md`「M2 已先留欄位」的過時宣稱。

## Non-Goals

（細節取捨見 design.md 的 Goals/Non-Goals。）

## Capabilities

### New Capabilities

- `voice-conversation`: 端到端語音通道——per-message voice 旗標、core 在終止回合產生 display+speech 雙通道輸出、server 只在終止回合送出 speech、終端用 OS 原生 STT/TTS 收音與朗讀；server 仍只進出文字，引擎在終端。

### Modified Capabilities

（無。goal-completion 既有的「Only the terminal turn replies, and speaks」涵蓋了「voice on 時的口語摘要」，本變更實作該行為的語音通道，不改其規格。）

## Impact

- 受影響 specs：新增 voice-conversation。修改：無。
- 受影響程式：
  - 修改：crates/fleety-protocol/src/lib.rs、crates/agent-core/src/agent.rs（run_turn/run_turn_streaming/TurnOutcome 與雙通道解析）、crates/fleety-server/src/conn.rs（drive_turn/drive_to_goal voice 整合）、crates/fleety-server/src/subagent.rs（drive_turn 呼叫點同步簽名）、crates/fleety-cli/src/main.rs、prompts/protocol.md、docs/tools.md、docs/env.md、docs/spec-v0.md
  - 新增：crates/fleety-cli/src/voice.rs（OS 原生 STT/TTS 終端整合）
  - 移除：無
- 關鍵驗收：協定欄位向後相容（舊客戶端不送 voice 仍正常）、voice off 不產 speech（不花 token）、voice on 僅終止回合帶 speech、agent-core 仍不依賴任何 fleety crate；workspace fmt + clippy -D + test 全綠。
