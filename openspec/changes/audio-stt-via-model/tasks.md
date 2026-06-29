## 1. 模型音訊能力告知 client

- [x] 1.1 在 crates/fleety-protocol/src/lib.rs 的 `ServerMsg::Welcome` 附加 `#[serde(default)] audio_input: bool`,並在 crates/fleety-server/src/conn.rs 帶入 provider 的音訊能力(來自 capability-aware-modality),交付 "The server advertises model audio-input capability to the client";對應設計「決定點在 client,能力經 Welcome 附加欄位取得」。先寫失敗測試(fleety-protocol):Welcome 帶 audio_input round-trip;舊 server 無欄位 → 解析為 false。

## 2. 壓縮與決策(純邏輯)

- [x] 2.1 在 crates/fleety-cli/src/voice.rs 實作 `voice_mode(audio_input, setting) -> Decision`(SendAudio / LocalStt)與 `within_limit(len, cap)`,交付 "Voice uses model audio when supported, else local STT" 的決策面與 "Sent audio is a compact mono encoding and size-bounded" 的上限判斷;對應設計「設定覆寫:FLEETY_VOICE_AUDIO = auto | on | off」與「支援音訊時:壓縮為 Opus(16kHz 單聲道)再送」。先寫失敗測試:用 spec example 決策表逐列驗證;within_limit 邊界。
- [x] 2.2 在 voice.rs 實作 `wav_bytes_mono16(pcm16_mono_16k) -> Vec<u8>`(16kHz 單聲道 → 記憶體中的 WAV bytes,mime `audio/wav`;沿用既有 WAV header 寫法),交付 "Sent audio is a compact mono encoding and size-bounded" 的編碼面;對應設計「支援音訊時:壓縮為 Opus(16kHz 單聲道)再送」與「編碼依賴隔離」(本階段用無依賴的 WAV;Opus 列為 design open question 的後續,避免原生 codec 依賴拖累跨平台 CI)。驗證:對一段已知 PCM 產出非空且 header 良好的 WAV bytes(單元測試)。

## 3. 接上 voice 流程 + 設定 + 文件

- [x] 3.1 在 crates/fleety-cli/src/main.rs 的 voice_chat 依 `voice_mode` 決策組 UserMessage:SendAudio → 壓縮音訊當 attachment、跳過本地 Whisper;LocalStt / 超限 / 編碼失敗 → 回退既有裝置端 Whisper 文字路徑,交付 "Voice uses model audio when supported, else local STT" 與超限回退;對應設計「不支援 / 離線 / 未知:維持裝置端 Whisper」。驗證:audio-capable 路徑送出含 audio attachment 的 UserMessage、不呼叫 whisper;不支援路徑沿用文字(單元/整合或程式碼審查 + 手動驗證)。
- [x] 3.2 [P] 把 `FLEETY_VOICE_AUDIO`(auto/on/off,預設 auto;未知值當 auto)登記到 typed config registry 並更新 docs/env.md,交付 "Voice transport mode is configurable";對應設計「設定覆寫:FLEETY_VOICE_AUDIO = auto | on | off」。驗證:`config list` 顯示該鍵;voice_mode 對 off 一律 LocalStt 的測試;docs/env.md 說明三值語意。
