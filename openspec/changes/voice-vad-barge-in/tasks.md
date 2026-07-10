## 1. VAD 純函式與狀態機（純函式化 VAD 狀態機以利測試）

- [ ] 1.1 在 `crates/fleety-cli/src/voice.rs` 新增 `rms(&[f32]) -> f32`，回傳窗內樣本的均方根能量；單元測試 `rms_silence_vs_full_scale` 驗證全零窗 RMS 近 0、滿幅窗 RMS 明顯較高（`cargo test -p fleety-cli`）。（design: VAD 方法：自寫能量門檻，不新增依賴）
- [ ] 1.2 新增 `VadConfig`、`enum VadDecision { WaitingForSpeech, Speaking, Stop(EndReason) }`、`enum EndReason { Silence, MaxDuration, StartTimeout }`、`VadState` 與 `observe(&mut self, window_rms, window_ms) -> VadDecision`，實作「語音活動端點偵測」狀態機（Voice-activity endpointed capture）；測試 `vad_endpoints_on_silence`（開口後靜音達 hangover → `Stop(Silence)`）、`vad_stops_at_max_duration`、`vad_start_timeout_without_speech`（從未偵測到語音 → `Stop(StartTimeout)`）。
- [ ] 1.3 新增 `onset_reached(consecutive_hot, needed) -> bool` 供 barge-in 使用（Barge-in during spoken playback）；測試 `onset_requires_sustained_energy` 驗證需連續達 `needed` 窗才為真、未達為假。

## 2. 串流式 VAD 擷取（串流式擷取取代固定秒數睡眠、設定旗標與回溯相容）

- [ ] 2.1 新增 `vad_config_from_env() -> VadConfig`，解析 `FLEETY_VAD_ENERGY`/`FLEETY_VAD_SILENCE_MS`/`FLEETY_VAD_MAX_MS`/`FLEETY_VAD_START_TIMEOUT_MS`（無效值退回預設，比照既有 `stt_seconds` 慣例）；測試 `vad_config_env_defaults_and_overrides` 驗證缺省值與覆寫值（設定旗標與回溯相容）。
- [ ] 2.2 改寫 `record_pcm16`：`FLEETY_VAD=on`（預設）時進入串流輪詢迴圈，分窗 `drain` 緩衝、算 RMS、餵 `VadState.observe` 直到 `Stop`，回傳累積 16kHz mono `i16`；`FLEETY_VAD=off` 時走原本固定 `sleep(FLEETY_STT_SECONDS)` 分支。契約：無輸入裝置／格式不支援仍回 `None` 不 panic。驗證方式：內容審閱確認 LocalStt 與 SendAudio 兩路徑共用此點且介面未變，並以 `cargo clippy -p fleety-cli`（`unwrap_used`/`expect_used` 為 warn）確認無違規。
- [ ] 2.3 更新 `capture_audio` 的使用者提示訊息，把「recording {secs}s」改為反映 VAD 聆聽中（開口即錄、停頓自動結束）；內容審閱確認提示與新行為一致、關閉 VAD 時仍顯示固定秒數提示。

## 3. Barge-in 播放（Barge-in：TTS 子行程 spawn + 麥克風 onset 監看）

- [ ] 3.1 新增 `enum SpeakOutcome { Completed, Interrupted, Unavailable }` 與 `speak_interruptible(text) -> SpeakOutcome`：以 `Command::spawn()` 起 TTS 子行程，並行監看麥克風、`onset_reached` 達標即 `child.kill()` 回 `Interrupted`，否則 `child.wait()` 回 `Completed`；`FLEETY_BARGE_IN=off`／無麥克風／無引擎時等同阻塞播放回 `Completed`/`Unavailable`。驗證：`speak_interruptible_empty_text_returns_unavailable_or_completed`（空字串不 spawn），並內容審閱確認 spawn/kill 失敗均不 panic。
- [ ] 3.2 保留現有 `speak(text) -> bool` 作為薄包裝（呼叫 `speak_interruptible` 並在 `Completed`/`Interrupted` 回 `true`），確保既有測試 `empty_text_is_not_spoken` 不回歸；`cargo test -p fleety-cli` 通過。

## 4. 主迴圈接線

- [ ] 4.1 在 `crates/fleety-cli/src/main.rs` 語音迴圈把 `voice::speak(&spoken)` 換成 `voice::speak_interruptible(&spoken)`，`Interrupted` 時及早返回讓外層轉入 VAD 擷取；內容審閱確認顯示文字與 attention 提示行為不變、barge-in 開關生效。

## 5. 驗證與文件

- [ ] 5.1 [P] 執行 `cargo test -p fleety-cli` 與 `cargo clippy -p fleety-cli --all-targets`，確認新純函式測試全過、無 clippy 違規、固定秒數回退路徑（`FLEETY_VAD=off`）行為與現行一致。
- [ ] 5.2 [P] 在 voice.rs 模組 doc comment 補上新增 env（`FLEETY_VAD`、`FLEETY_VAD_ENERGY`、`FLEETY_VAD_SILENCE_MS`、`FLEETY_VAD_MAX_MS`、`FLEETY_VAD_START_TIMEOUT_MS`、`FLEETY_BARGE_IN`）的說明與預設值；內容審閱確認與實作解析一致。