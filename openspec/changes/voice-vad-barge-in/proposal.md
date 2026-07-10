## Why

語音模式用固定秒數盲錄（`FLEETY_STT_SECONDS`），使用者不知道何時該講、何時被截斷；且 TTS 唸回覆時 `speak()` 阻塞整個迴圈，無法打斷（barge-in）插話或跳過。

## What Changes

- 用語音活動偵測（VAD）取代固定秒數擷取：一開始就聆聽，偵測到說話能量才算開始，尾端靜音超過門檻即自動斷句結束錄音；並加上最長時限與「開口前逾時」上限。
- 把 `record_pcm16` 的固定 `sleep` 改成串流式擷取迴圈：邊錄邊分窗計算能量，餵給一個純函式 VAD 狀態機決定何時停。LocalStt 與 SendAudio 兩條路徑共用這個新擷取點。
- TTS 播放中偵測使用者開口即停止播放並轉入聆聽（barge-in）：`speak` 改為 spawn 子行程並回傳可觀察結果，另一路監看麥克風 onset，偵測到就 kill 子行程。
- 新增設定旗標：`FLEETY_VAD`（on/off，預設 on，off 回到固定秒數）、`FLEETY_VAD_ENERGY`、`FLEETY_VAD_SILENCE_MS`、`FLEETY_VAD_MAX_MS`、`FLEETY_VAD_START_TIMEOUT_MS`、`FLEETY_BARGE_IN`（on/off，預設 on）。VAD 關閉時 `FLEETY_STT_SECONDS` 續用。
- 把 VAD 的能量計算、狀態機、onset 判定抽成純函式，讓沒有麥克風硬體的 CI 也能單元測試。

## Non-Goals

- 不引入外部 VAD 函式庫或 ML 模型，也不新增 crate 依賴。VAD 用自寫的 RMS 能量門檻（決策見 design；若日後要換更準的方法，屬後續變更）。
- 不做聲學回音消除（AEC）。喇叭聲被麥克風收到可能造成 barge-in 誤觸發，本次只用能量門檻與「需持續數個窗」的條件緩解，並提供旗標關閉，不做完整 AEC。
- 不改動 wire protocol、不改 server / agent-core，純終端行為。
- 不改行動裝置或其他非終端 client 的輸入方式。
- 不做 full-duplex 邊講邊送串流到模型；barge-in 只負責停播並轉入既有的單次擷取流程。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `voice-conversation`: 新增兩個 requirement — 以 VAD 端點偵測取代固定秒數擷取，以及 TTS 播放中的 barge-in 打斷。既有 requirement 措辭不動。

## Impact

- Affected specs: voice-conversation
- Affected code:
  - Modified: crates/fleety-cli/src/voice.rs
  - Modified: crates/fleety-cli/src/main.rs