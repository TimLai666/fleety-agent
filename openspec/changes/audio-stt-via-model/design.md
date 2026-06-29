## Context

voice.rs 在裝置端錄 16kHz 單聲道 PCM16 WAV,呼叫 whisper-cli 轉文字後刪掉 WAV,只把文字以 `UserMessage{ voice: true, attachments: [] }` 送出;server 從不碰音訊。但 WireAttachment 與兩個 provider 都已支援音訊 attachment(openai.rs input_audio、gemini.rs inline_data)。模型是否支援音訊輸入由 capability-aware-modality 在 server 端得知;client(CLI)有麥克風與 Whisper 但不知道 server 的模型能力。Welcome 最近已用附加欄位帶 server_version(協定相容模式)。約束:forbid unsafe;never-crash;agent-core 不依賴 fleety crate(本變更主要在 cli/protocol/server,不動 agent-core)。

## Goals / Non-Goals

**Goals:**

- 模型支援音訊輸入時,語音以壓縮音訊直送模型(免本地 Whisper),由模型一次轉錄+回應。
- 模型不支援 / 離線 / 未知時,維持既有裝置端 Whisper → 文字(零退步)。
- client 能得知模型音訊能力以做決定;作業者可用設定覆寫。
- 音訊壓縮且有大小/長度上限。

**Non-Goals:**

- 不移除裝置端 Whisper(它是 fallback,且離線時唯一選擇)。
- 不改 server 端做 STT(server 無 Whisper);決定點在 client。
- 不改既有 audio attachment 的 provider 路由(沿用)。
- 不做即時串流音訊(一次一段語音 → 一個 attachment)。

## Decisions

### 決定點在 client,能力經 Welcome 附加欄位取得

模型音訊輸入能力由 server 在 `Welcome` 以**附加欄位**告知(沿用 server_version 模式,例如 `audio_input: bool`,舊 client 忽略、舊 server 給預設 false)。voice client 連線收到 Welcome 後得知能力;這是唯一乾淨且 client 能拿到的訊號(client 無法自己知道 server 的模型設定)。理由:把「模型能不能聽」這個只有 server 知道的事實傳給有麥克風的一端。

### 設定覆寫:FLEETY_VOICE_AUDIO = auto | on | off

`auto`(預設):依 Welcome 的 audio_input 決定;`on`:強制送音訊(作業者確知模型可聽);`off`:一律本地 Whisper(沿用現行)。理由:auto 自動最佳化,on/off 給作業者明確控制與除錯途徑。

### 支援音訊時:壓縮為 Opus(16kHz 單聲道)再送

擷取的 16kHz 單聲道 PCM 編碼為 **Opus**(語音壓縮比最佳),包進 audio attachment(mime `audio/ogg` 或 `audio/opus`,format 對應)經既有 UserMessage attachments 送出;不跑本地 Whisper。設一個**長度/大小上限**(可設定),超過則截斷或回退本地 Whisper。理由:Opus 對語音的體積/品質最佳,16kHz 單聲道已足夠語音辨識。

### 不支援 / 離線 / 未知:維持裝置端 Whisper

`audio_input=false` 或 `FLEETY_VOICE_AUDIO=off` 或無法取得能力 → 走現行 voice.rs 路徑(本地 whisper-cli → 文字 → UserMessage)。理由:零退步、離線可用。

### 編碼依賴隔離

音訊編碼(Opus)放在 voice 模組一個薄封裝,輸入 16kHz 單聲道 PCM、輸出壓縮 bytes + mime/format;PCM→容器組裝/大小判斷做成可測的純邏輯(編碼器呼叫本身手動驗證)。理由:把唯一的新(可能是原生)依賴與 I/O 隔離,核心決策與大小邏輯可測。

## Implementation Contract

**行為(Behavior):**

- Welcome.audio_input=true 且 FLEETY_VOICE_AUDIO!=off:`fleety voice` 擷取語音 → 壓縮 → 以 audio attachment 送出(無本地 Whisper 呼叫),模型回應即視為轉錄+答覆。
- Welcome.audio_input=false 或 =off 或未知:走現行裝置端 Whisper → 文字 UserMessage(行為與現況相同)。
- FLEETY_VOICE_AUDIO=on:不論 Welcome 一律送音訊。
- 壓縮輸出超過大小上限:截斷到上限或回退本地 Whisper(擇一,於 spec 明訂)。
- 舊 server(無 audio_input 欄位):client 視為 false → 走 Whisper(相容)。

**介面 / 資料形狀:**

- protocol:`ServerMsg::Welcome` 附加 `#[serde(default)] audio_input: bool`。
- server conn.rs:Welcome 帶入 `provider.capabilities()` 的 audio 支援(來自 capability-aware-modality)。
- voice.rs:`encode_opus(pcm16_mono_16k: &[i16]) -> Result<(Vec<u8>, mime, format)>`;`fn within_limit(len, cap) -> bool`(純);決定函式 `voice_mode(audio_input: bool, setting: VoiceAudio) -> Decision{ SendAudio | LocalStt }`(純,可測)。
- main.rs voice_chat:依 Decision 組 `UserMessage`:SendAudio → attachments=[audio];LocalStt → 既有文字路徑。
- config:`FLEETY_VOICE_AUDIO`(auto/on/off)。

**失敗模式:**

- 麥克風/編碼失敗 → 回退本地 Whisper;再失敗 → 既有「改用打字」fallback;never panic。
- 設定值非 auto/on/off → 視為 auto。

**驗收標準(Acceptance):**

- 單元測試:`voice_mode` 對 (audio_input × setting) 的決策表(auto+true→SendAudio、auto+false→LocalStt、on→SendAudio、off→LocalStt)。
- 單元測試:`within_limit` 邊界;Welcome.audio_input 附加欄位 round-trip + 舊 server(無欄位)→ false(協定相容測試,放 fleety-protocol)。
- 既有 voice 測試與 server Welcome 測試全綠。
- clippy -D 乾淨、env 測試單執行緒;Opus 編碼與真模型轉錄為手動驗證。

**範圍邊界:**

- In scope:Welcome 音訊能力欄位、client 決策(auto/on/off)、Opus 壓縮+大小上限、與既有 audio attachment 的銜接、fallback、設定與文件。
- Out of scope:server 端 STT、即時串流音訊、移除 Whisper、改 audio attachment 的 provider 路由。

## Risks / Trade-offs

- [Opus 編碼器多為原生依賴(libopus 綁定)] → 隔離在薄封裝;若不接受原生依賴,退而求其次用體積較大但純 Rust 的編碼或限制長度送 WAV(列 Open Question)。這是本變更的主要風險點。
- [並非所有「多模態」模型都吃音訊(例如 Claude 目前不收音訊)] → 由 capability-aware-modality 的能力如實判定;Welcome 只在真的支援時報 true。
- [音訊 payload 過大] → 16kHz 單聲道 + Opus + 大小上限;超限截斷或回退 Whisper。
- [離線] → audio_input 不可得 → 走 Whisper。

## Migration Plan

- 附加式:舊 server 無 audio_input → client 當 false → 走現行 Whisper(完全相容)。預設 auto;要關閉設 `FLEETY_VOICE_AUDIO=off`。
- 無資料遷移、不改既有 attachment 格式。

## Open Questions

- Opus 原生依賴是否可接受;若否,改用哪種壓縮(純 Rust 編碼 / 限長 WAV)——apply 前定。
- 超過大小上限時「截斷」還是「回退本地 Whisper」——初版傾向回退 Whisper(較不破壞語意)。
- 音訊能力是否改用更一般的 modalities 集合(而非單一 audio_input bool)隨 Welcome 帶——初版用單一 bool 夠用。
