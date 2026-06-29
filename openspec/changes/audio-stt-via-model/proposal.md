## Why

語音輸入目前固定在裝置端用 whisper.cpp 轉文字(voice.rs 錄 16kHz 單聲道 WAV → 本地 whisper-cli → 只送文字),即使連的是支援音訊輸入的多模態模型也一樣。但底層協定與 provider 其實已支援音訊 attachment(openai.rs 的 input_audio、gemini.rs 的 inline_data)。若模型本身能聽音訊,直接把(壓縮過的)音訊送給模型一次轉錄+回應,可省掉裝置端 Whisper 的安裝與延遲,品質也常更好。前提是要知道模型支不支援音訊(由 capability-aware-modality 提供),且音訊要壓縮、不要太大。

## What Changes

- voice 流程依「模型是否支援音訊輸入」二擇一:**支援** → 擷取音訊、**壓縮**(語音用 Opus,16kHz 單聲道)、當 audio attachment 直送模型(沿用既有 input_audio / inline_data 路由),不跑本地 Whisper;**不支援 / 離線 / 未知** → 維持現行裝置端 Whisper → 送文字(fallback)。
- 模型音訊能力以**附加式**經 Welcome 告知 client(沿用 server_version 的加欄位模式),voice client 據此決定;另提供設定 `FLEETY_VOICE_AUDIO`(auto / on / off)覆寫,預設 auto。
- 壓縮有大小/長度上限,避免 payload 過大。

## Non-Goals

(本變更會建立 design.md,Non-Goals 寫在 design 的 Goals/Non-Goals 一節。)

## Capabilities

### New Capabilities

- `audio-stt-via-model`: 當模型支援音訊輸入時,語音改以壓縮音訊直送模型(免本地 Whisper);否則 fallback 到既有裝置端 STT。含:模型音訊能力的 client 取得(Welcome 附加欄位)+ 設定覆寫、Opus 壓縮與大小上限、與既有 audio attachment 路由的銜接。

### Modified Capabilities

(none)

## Impact

- Affected specs: audio-stt-via-model(新)
- Affected code:
  - Modified:
    - crates/fleety-protocol/src/lib.rs(Welcome 附加 audio-input 能力欄位)
    - crates/fleety-server/src/conn.rs(Welcome 帶入解析出的模型音訊能力)
    - crates/fleety-cli/src/voice.rs(擷取→壓縮;依能力決定送音訊或本地 Whisper)
    - crates/fleety-cli/src/main.rs(voice_chat 依能力/設定組 UserMessage:audio attachment 或文字)
    - crates/fleety-tools/src/config.rs(FLEETY_VOICE_AUDIO 設定鍵)
    - crates/fleety-cli/Cargo.toml(音訊編碼依賴)
    - docs/env.md(記錄 FLEETY_VOICE_AUDIO)
