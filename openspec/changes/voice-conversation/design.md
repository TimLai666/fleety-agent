## Context

語音是 spec-v0 §11 的 M7（post-v0）能力，但協定層完全沒做：`fleety-protocol` 的 `ClientMsg::UserMessage` 無 voice 旗標、`ServerMsg::Assistant` 無 speech 欄位；`fleety-cli` 只有 audio 輸入附件，無 STT/TTS。spec-v0 §12「欄位 M2 已先留」與現況不符。

既有可掛載的接點：goal-completion 在 `conn` 的 `drive_turn`/`drive_to_goal` 已有 `emit_terminal`——只有終止回合（complete_goal/ask_user/單回合）emit 使用者回覆。protocol.md 的 Output Channels 段已描述「display + speech 兩通道、voice on 才產 speech」的行為契約，但 runtime 未實作。本變更把這條通道做出來，server 仍只進出文字，STT/TTS 引擎在終端。

使用者已拍板：(1) 範圍＝完整語音對話（STT 輸入 + TTS 輸出 + 協定 + agent 產口語）；(2) 口語產生＝模型同回合雙通道（終止回合模型同時輸出 display + speech，一次 LLM 呼叫）；(3) 引擎＝OS 原生（Windows SAPI、macOS say/Speech、Linux speech-dispatcher）。

## Goals / Non-Goals

**Goals:**
- per-message voice 旗標貫通 CLI → 協定 → core；voice on 才要求並產生口語，voice off 不花 token。
- core 在終止回合產生 display + speech 雙通道，server 只在終止回合把 speech emit 出去（接 goal-completion 的 emit_terminal）。
- 終端用 OS 原生引擎收音（STT）與朗讀（TTS），引擎缺失或失敗優雅退回純文字。
- 協定向後相容：欄位皆 serde default/skip，舊客戶端不送 voice、不認 speech 仍正常，`PROTOCOL_VERSION` 不 bump。

**Non-Goals:**
- 不自帶雲端或本地模型引擎（whisper/piper/雲端 API）——只用 OS 原生。
- 不改 server「只進出文字」原則；STT/TTS 純在終端。
- 不改 `run_turn` 的 tool-loop 邏輯，只加 voice-aware 的系統 prompt 與最終訊息的雙通道解析。
- 不改非 voice 回合與既有單通道呼叫點（subagent、recovery）的行為。
- 不做喚醒詞、串流即時語音、打斷（barge-in）等進階語音 UX。

## Decisions

### 協定欄位：UserMessage.voice 與 Assistant.speech，向後相容不 bump

`ClientMsg::UserMessage` 加 `voice: bool`（`#[serde(default)]`，預設 false）；`ServerMsg::Assistant` 加 `speech: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`，僅終止回合且 voice on 時帶值）。speech 與 display 同屬一個終止回覆，放同一個 `Assistant` 幀讓客戶端原子取得、不必跨幀配對。

**替代方案：** 新增獨立 `ServerMsg::Speech` 幀——否決，因為要處理與 `Assistant` 的順序與配對、客戶端要 buffer，複雜且無好處。欄位用 serde default/skip 即可向後相容，故不 bump `PROTOCOL_VERSION`（仍為 0）。

### 雙通道輸出契約：模型在終止回覆附口語分隔區塊，core 解析切分

voice on 的回合，系統 prompt 指示模型在最終回覆「先輸出正常 display 內容，最後另起一行以固定哨符標記一段純口語版」，哨符為不易誤撞的標記（例如獨佔一行的 `⟦SPEECH⟧`）。core 在回合結束後對模型最終訊息做一次切分：哨符前為 display（持久化與 emit 的權威文字），哨符後為 speech。找不到哨符（模型沒給）時 speech = None，display = 全文，不報錯。voice off 時系統 prompt 不含口語要求，core 不切分、不產 speech。

**替代方案：** (a) 要求模型輸出結構化 JSON——否決，破壞既有純文字 output 契約與 AssistantDelta 串流顯示，且 provider 相容性差。(b) server 終止回合對 display 額外呼叫一次模型濃縮——使用者已否決（多一次呼叫、口語與 display 可能不同步）。

**串流取捨：** `AssistantDelta` 仍以原 chunk 串流（best-effort 進度）；權威切分在 core 對「最終訊息」做，最終 `Assistant.text` 必為乾淨 display。哨符後的口語可能短暫漏進 delta，緩解：哨符選用極不易誤撞字串，終端在偵測到哨符行後停止往顯示區追加。

### voice 旗標經 LoopConfig 傳入 core 並驅動系統 prompt

`LoopConfig` 加 `voice: bool`（預設 false），由 `run_turn`/`run_turn_streaming` 讀。voice on 時組裝系統 prompt 追加「雙通道輸出與哨符」說明段；voice off 時不追加（零額外 token）。`TurnOutcome` 加 `speech: Option<String>` 承載切分結果。既有呼叫點（subagent、recovery、非 voice 一般回合）一律傳 voice=false，行為不變。

**替代方案：** 用 env 旗標——否決，voice 是 per-message/per-session，不是 server 全域。

### server 僅在終止回合 emit speech

`drive_turn` 既有 `emit_terminal` 之外，接收本回合是否 voice；`LoopConfig.voice` 依此設定。`drive_to_goal` 把使用者訊息的 voice 旗標傳入每個回合的 `drive_turn`，但只有終止回合（`emit_terminal` 對應的那次 emit）把 `TurnOutcome.speech` 放進 `ServerMsg::Assistant.speech`。中間續做回合不產口語（系統 prompt 仍可含哨符要求，但其 speech 不被 emit；為省 token，中間回合傳 voice=false，只有預期會終止的回合……取捨見下）。ask_user 的問題走終止回合，同樣帶口語版。

**中間回合 token 取捨：** 因為無法預知哪個回合會終止，採「voice on 的訊息：每個回合都以 voice=true 跑（模型都會附口語），但只有終止回合 emit speech」；中間回合的口語被丟棄。原本考慮以 `FLEETY_VOICE_INTERMEDIATE` 讓中間回合不產口語省 token，但實作時確認做不到——無法預知哪個回合是終止回合，若中間回合一律 voice=false 則真正的終止回合也會沒口語（除非預測，或對終止回合再呼叫一次模型，後者已被否決）。故不引入該 env，採單純規則：voice on 的訊息每回合都產口語、只 emit 終止回合的。日後若要省中間 token 需另設計（例如偵測 complete_goal/ask_user 後重跑終止回合），不在本變更範圍。

### 終端 OS 原生 STT/TTS 與優雅退回

`fleety-cli` 新增 voice 模組：TTS 把收到的 `Assistant.speech` 交給 OS 引擎朗讀（macOS `say`、Windows SAPI、Linux `spd-say`/speech-dispatcher）；STT 把語音輸入轉文字並送 `voice: true` 的 `UserMessage`。引擎以 `std::process::Command` 呼叫外部工具；二進位/服務不存在或失敗時，回退為純文字（TTS 略過朗讀、STT 提示改用打字），永不 crash。

**OS 原生 STT 缺口（誠實標明，實作時擴大認知）：** headless CLI 的 OS 原生語音輸入在三平台都偏弱——只有 Windows 可經 System.Speech 做 best-effort 自由聽寫；macOS 與 Linux 沒有可靠的無頭 OS 聽寫 CLI。故 STT 僅 Windows best-effort，macOS/Linux 一律退回「請改用打字」的明確提示。TTS（朗讀）三平台都用 OS 原生（say／SAPI／spd-say）正常運作。雲端/本地模型 STT（如 whisper.cpp）不在本變更範圍；要在 mac/Linux 真正做到語音輸入需另立案改 STT 引擎方向。

**替代方案：** 雲端 API 或內嵌本地模型——使用者已選 OS 原生。

### 文件與既有契約同步

`prompts/protocol.md` 的 Output Channels 段從「行為已描述、runtime 未實作」更新為實作後狀態（voice 旗標、speech 欄位、終止回合產出）；修正 `docs/spec-v0.md` §11/§12「speech 欄位與 voice 旗標 M2 已先留」的過時宣稱為「於本變更實作」並更新 M7 狀態。voice 不新增 server env（per-message 旗標即可，且 `FLEETY_VOICE_INTERMEDIATE` 已確認做不到而捨棄）、也不新增 agent 工具，故 `docs/env.md`、`docs/tools.md` 無需變更。`fleety voice` 子命令在 CLI 端文件提及。

## Implementation Contract

**Behavior:** 使用者在 voice 模式下對 `fleety-cli` 說話 → CLI 用 OS STT 轉文字，送 `UserMessage{voice:true}` → agent 照常驅動目標；達成（complete_goal）或必問（ask_user）的終止回合，模型同時產 display 與口語版 → server emit `Assistant{text, speech, seq}` + `Done` → CLI 顯示 text 並用 OS TTS 朗讀 speech。非終止（續做）回合不朗讀。voice off 時行為與現況完全相同（無 speech、無額外 token）。

**Interfaces / data shapes:**
- `ClientMsg::UserMessage` 新欄位 `voice: bool`（serde default=false）。
- `ServerMsg::Assistant` 新欄位 `speech: Option<String>`（serde default/skip）。
- `agent_core::LoopConfig` 新欄位 `voice: bool`（預設 false）。
- `agent_core::TurnOutcome` 新欄位 `speech: Option<String>`。
- 雙通道哨符：voice on 時系統 prompt 約定的口語分隔標記（獨佔一行）；core 以此切分最終訊息為 (display, speech)。
- `conn::drive_turn` 既有簽名再加 voice 來源（由 `drive_to_goal` 從 `UserMessage.voice` 傳入）；終止回合 emit 時填入 `Assistant.speech`。
- `fleety-cli` voice 模組對外行為：`speak(text)`（TTS）與 `listen() -> Option<String>`（STT），各自封裝 OS 命令。

**Failure modes:** 缺哨符 → speech=None、display=全文、不報錯。TTS/STT 引擎缺失或失敗 → 退回純文字、印出可行動提示、永不 crash。Linux STT → 明確「不支援，請打字」。舊客戶端不送 voice → 視為 false；不認 speech 欄位 → serde 略過。協定不 bump，混版相容。

**Acceptance criteria:**
- fleety-protocol：UserMessage 帶/不帶 voice、Assistant 帶/不帶 speech 皆 serde round-trip 過；不含 voice/speech 的舊 JSON 仍能反序列化（向後相容測試）。
- agent-core：voice=false 時 TurnOutcome.speech=None 且系統 prompt 不含口語段；voice=true 且模型輸出含哨符時，display 不含哨符與其後內容、speech= 哨符後文字；模型未給哨符時 speech=None、display 完整（單元測試，MockProvider）。
- fleety-server：voice on 的多回合迴圈只有終止回合的 `Assistant` 帶 speech，中間回合不帶（以 out 接收端斷言）；voice off 的回合 `Assistant.speech` 為 None。
- fleety-cli：voice 模組在引擎缺失時退回純文字不 crash（可注入/模擬命令失敗的測試或以 feature/平台條件驗證）。
- agent-core 仍不依賴任何 fleety crate（cargo tree）。
- cargo fmt + clippy --workspace -D warnings + test --workspace 全綠。

**Scope boundaries:**
- In：協定兩欄位、LoopConfig/TurnOutcome 雙通道與哨符解析、voice-aware 系統 prompt 段、conn 的 voice 傳遞與終止回合 speech emit、fleety-cli 的 OS 原生 STT/TTS 與退回、文件同步、測試。
- Out：雲端/本地模型引擎、Linux 原生 STT、喚醒詞/串流語音/barge-in、run_turn tool-loop 改動、語音以外的 UX。

## Risks / Trade-offs

- [哨符被模型忽略或誤用] → core 容錯：無哨符即 speech=None；系統 prompt 給明確範例；哨符選極不易誤撞字串。
- [哨符後口語短暫漏進 AssistantDelta 顯示] → 權威切分在最終 `Assistant.text`（必乾淨）；終端在偵測哨符行後停止追加顯示。
- [中間回合產口語浪費 token] → 接受；無法在不預測終止回合或不多呼叫一次模型的前提下省掉（`FLEETY_VOICE_INTERMEDIATE` 因此捨棄）。日後若需省 token 另設計。
- [mac/Linux 無可靠無頭 OS STT] → STT 僅 Windows best-effort，其餘明確退回打字；TTS 三平台皆可。真正的 mac/Linux 語音輸入需另立案改引擎。
- [OS 引擎跨平台行為參差／不存在] → 一律以外部命令呼叫並優雅退回純文字，never-crash；缺引擎給安裝提示。
- [協定演進] → 欄位向後相容、不 bump；若日後改雙通道格式再評估。
