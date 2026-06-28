## Context

voice-conversation 已上線：協定 `UserMessage.voice` + `Assistant.speech`、agent-core 雙通道哨符切分（`SPEECH_SENTINEL`，見 agent.rs）、conn 終止回合 emit speech、fleety-cli OS 原生 TTS + best-effort Windows STT。封存時標注兩缺口：mac/Linux 無真正 STT、device deixis 只有 prompt 行為描述無協定/終端實作。本變更補這兩塊。使用者已拍板（最佳假設，apply 前可改）：STT 用本地 whisper.cpp（可設定命令）；device deixis 以結構化 attention hint 承載，core 解析比照 speech 哨符。

## Goals / Non-Goals

**Goals:**
- 終端跨平台語音輸入：錄麥克風→可設定轉寫命令（預設 whisper.cpp）→送 `voice:true` 訊息；缺引擎/失敗優雅退回（Windows System.Speech；其餘打字），永不 crash。server 仍只進出文字。
- voice on 終止回合可附結構化 attention hint（device + 看什麼 + 可選 url/path），core 解析、協定向後相容承載、終端呈現/開啟。
- agent-core 維持 host-free（attention 解析在 agent.rs，用 core 自有型別；音訊/STT 全在 fleety-cli）。

**Non-Goals:**
- 不做雲端 STT（替代方案，使用者選本地）；不改 TTS（已可用）。
- 不改 server「只進出文字」原則、不改 goal/voice/skill-learning-loop 機制本身。
- attention hint 不做自動導航/自動執行——只把「看哪台、看什麼」傳給終端呈現，是否開啟由終端/使用者決定。

## Decisions

### 跨平台 STT：終端錄音 + 可設定轉寫命令（預設 whisper.cpp）

`fleety-cli` 的 `voice::listen` 改為：先錄一段麥克風音訊到暫存 WAV，再以可設定的轉寫命令（`FLEETY_STT_CMD`，預設嘗試 whisper.cpp，例如 `whisper-cli -m <model> -f <wav> -otxt`；模型路徑 `FLEETY_STT_MODEL`）轉成文字，回傳。轉寫命令以 `std::process::Command` 呼叫外部工具——whisper.cpp 是獨立二進位、離線、跨平台，不綁進 binary。

**替代方案：** 雲端 STT（品質最佳但要金鑰/網路/隱私，使用者已否決）；純 OS 原生（mac/Linux 無可靠無頭 STT，voice-conversation 已證實不可行）。

### 麥克風擷取：採 cpal 跨平台音訊擷取（使用者已拍板）

**使用者已選 `cpal`**：`fleety-cli` 直接用 cpal crate 在程式內擷取麥克風（Windows WASAPI／macOS CoreAudio／Linux ALSA），開箱即用、不要求使用者另裝錄音程式。擷取後寫成 whisper.cpp 要的 16 kHz mono 16-bit WAV 暫存檔（盡量請求 16 kHz mono 的輸入設定；若裝置不支援則以裝置原生取樣率擷取再降頻到 16 kHz）。WAV 標頭手寫（44-byte PCM header，不額外引入 WAV 套件）。錄音長度以 env 設定（預設固定秒數；靜音偵測列為後續）。新增依賴：`cpal`（fleety-cli 的 `Cargo.toml`）。

**替代方案（未採）：** 呼叫外部錄音命令（`ffmpeg`/`sox`/`arecord`）——零新 Rust 相依但要求使用者環境先裝該程式、各平台命令不一；使用者選了 cpal 的開箱即用，故不採。

### STT 退回鏈與 never-crash

`listen` 的順序：若轉寫命令可用（env 設定或偵測到 whisper.cpp）→ 錄音+轉寫；失敗或不可用 → Windows 退回 System.Speech（既有）→ 其餘平台回 None 並提示改打字。任何一步（無麥克風、命令缺失、轉寫非零退出、暫存檔 I/O 失敗）都回 None/提示、永不 panic。暫存 WAV 用後即刪。

### device deixis：attention hint 的協定形狀與向後相容

`fleety-protocol` 新增 `AttentionHint { device: String, look_at: String, url: Option<String> }`；`ServerMsg::Assistant` 加 `attention: Option<AttentionHint>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`，向後相容、不 bump）。僅 voice on 的終止回合可能帶值。終端收到後印出「看 <device> 的 <look_at>」，有 url 則提示/開啟。

**替代方案：** 獨立 `ServerMsg::Attention` 幀——否決，attention 與該回合回覆同屬一次終止輸出，放同幀讓終端原子取得、免配對（與 speech 同理）。

### attention hint 的模型輸出格式與 core 解析（比照 speech 哨符）

voice on 時系統 prompt（agent-core 既有 voice 注入處）追加：可選在 speech 之後再附一行哨符 `⟦ATTENTION⟧`，其後一行為 `device=<id>; look=<what>; url=<optional>`。`agent-core` 在切出 display/speech 後，再從尾段解析此區塊為 core 自有型別 `agent_core::AttentionHint { device, look_at, url }`，放進 `TurnOutcome.attention: Option<AttentionHint>`。無哨符→None。**host-free 維持**：core 不引入 fleety 型別；`conn` 把 `agent_core::AttentionHint` 對映成 `fleety_protocol::AttentionHint` 填入 `Assistant.attention`（兩個小結構各自定義、conn 做對映，比照既有跨層慣例）。

### 與既有 voice/goal/skill-learning-loop 的互動邊界

attention 只在 voice on 且終止回合產生與 emit（比照 speech，掛在 `emit_terminal`）；voice off 不解析、不產（零成本）。反思回合（skill-learning-loop）voice=false → 不產 attention。STT 只影響 fleety-cli 輸入端，不動 server/agent-core 的回合邏輯。

## Implementation Contract

**Behavior:** voice 模式下，mac/Linux/Windows 終端皆可講話：whisper.cpp（或設定的命令）可用時轉成文字送出；不可用時退回打字（Windows 另有 System.Speech）。agent 在 voice on 終止回合，除了顯示與口語，可再給一個「看哪台裝置的什麼」的 attention hint；終端據此提示或開啟。未啟用 voice、或無 attention/STT 引擎時，行為與現況一致、永不 crash。

**Interfaces / data shapes:**
- env：`FLEETY_STT_CMD`（轉寫命令樣板，預設 whisper.cpp）、`FLEETY_STT_MODEL`（模型路徑）、`FLEETY_STT_SECONDS`（cpal 錄音秒數，預設一個合理值）。麥克風擷取由 cpal 處理，無錄音命令 env。
- `fleety_protocol::AttentionHint { device: String, look_at: String, url: Option<String> }`；`ServerMsg::Assistant` 加 `attention: Option<AttentionHint>`（serde default/skip）。
- `agent_core::AttentionHint { device, look_at, url }`（host-free）；`TurnOutcome` 加 `attention: Option<AttentionHint>`；voice 注入的系統 prompt 增 `⟦ATTENTION⟧` 約定；agent.rs 解析。
- `conn::drive_turn`/`TurnReply` 帶出 attention，終止回合對映成協定型別填入 `Assistant.attention`（比照 speech）。
- `fleety-cli`：`voice::listen` 走錄音+轉寫+退回；voice 迴圈在收到 `Assistant.attention` 時呈現/開啟。

**Failure modes:** 無麥克風/錄音命令/轉寫命令/模型 → listen 回 None + 提示打字，不 crash。轉寫空輸出 → None。attention 哨符缺失或格式不符 → attention=None、display/speech 不受影響。舊端不認 attention 欄位 → serde 略過。暫存音訊檔務必清除。

**Acceptance criteria:**
- fleety-cli 單元測試：錄音/轉寫命令不存在時 `listen` 回 None 不 crash（以 bogus 命令模擬，比照既有 voice 退回測試）。
- agent-core 單元測試：voice on 且模型輸出含 `⟦SPEECH⟧`+`⟦ATTENTION⟧` → display/speech/attention 三者正確切分；無 attention 哨符 → attention=None；voice off → attention=None。
- fleety-protocol 單元測試：`Assistant` 帶/不帶 attention 皆 serde round-trip；缺 attention 的舊 JSON 仍反序列化（向後相容）。
- conn 單元測試：voice on 終止回合把 attention 填入 `Assistant.attention`；voice off 為 None。
- 內容審查：docs/env 有 STT env、prompts 有 attention 用法；agent-core 仍無 fleety-* 依賴（cargo tree）。
- cargo fmt + clippy --workspace -D warnings + test --workspace 全綠。

**Scope boundaries:**
- In：fleety-cli STT 錄音+轉寫+退回、attention 協定欄位與型別、agent-core attention 解析（host-free）、conn 帶出 attention、終端呈現、env、prompts、docs、測試。
- Out：雲端 STT、TTS 改動、自動導航/自動開啟的決策邏輯、goal/voice/skill-learning-loop 機制改動、agent-core 引入 fleety 依賴或音訊相依。

## Risks / Trade-offs

- [whisper.cpp 未安裝/模型缺失] → 可設定命令 + 偵測；缺則退回打字/OS STT，明確提示安裝；永不 crash。
- [新增 cpal 依賴 + 跨平台建置面] → 使用者已選 cpal（開箱即用）；cpal 為成熟跨平台 crate；錄音失敗（無裝置/權限）優雅退回打字，永不 crash。
- [裝置取樣率非 16 kHz、需降頻] → 盡量請求 16 kHz mono 輸入；不支援則以原生率擷取後降頻到 16 kHz 再寫 WAV。
- [錄音結束策略（固定秒數截斷/過長）] → `FLEETY_STT_SECONDS` 可調，後續可加靜音偵測；先求可用。
- [attention 哨符被模型忽略或格式不符] → core 容錯：無/壞即 attention=None，不影響 display/speech。
- [協定演進] → attention 欄位向後相容、不 bump；日後改格式再評估。
