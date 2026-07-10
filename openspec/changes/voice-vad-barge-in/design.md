## Context

`crates/fleety-cli/src/voice.rs` 的 `record_pcm16(seconds)` 目前開 cpal 串流後 `std::thread::sleep(Duration::from_secs(seconds))`（voice.rs:255），再一次取走整個緩衝。`record_wav`（LocalStt）與 `capture_audio`（SendAudio）都經此路徑，所以固定秒數的缺陷是單一根因。TTS 端 `speak_impl` → `run_status`（voice.rs:336）以阻塞的 `.status()` 執行 OS 引擎（macOS `say`、Windows SAPI via PowerShell、Linux `spd-say`），`main.rs:962` 在收訊迴圈裡同步呼叫 `voice::speak`，因此播放期間無法監看麥克風、無法打斷。

現況依賴只有 `cpal = "0.15"`，沒有任何 VAD 函式庫。VAD 用什麼方式是本變更唯一需要拍板的技術決策。

## Goals

- 用靜音端點偵測自動斷句，取代固定秒數；使用者一開口就錄、停下就送。
- TTS 播放中偵測到使用者開口即停播並轉入聆聽。
- 沒有麥克風／引擎時一律優雅退化，絕不 crash（延續既有 never-crash 約束）。
- VAD 核心邏輯純函式化，讓無音訊硬體的 CI 可測。

## Non-Goals

- 不新增依賴、不引入 ML VAD。
- 不做聲學回音消除、不做 full-duplex 串流。
- 不動 protocol / server / agent-core。

## Decisions

### VAD 方法：自寫能量門檻，不新增依賴

用每個短窗（約 20–40ms）的 RMS 能量對比門檻 `FLEETY_VAD_ENERGY` 判定「有無說話」。理由：cpal 已把樣本正規化成 `f32`（voice.rs:230、243），算 RMS 是幾行純運算；turn-taking 只需區分「有人講／靜音」，不需音素級精度。替代方案 `webrtc-vad`（C bindings）或 Silero（ONNX 模型）更準但都要新增依賴與跨平台建置負擔，與 Non-Goals 抵觸。成立前提：一般安靜環境、單一近場麥克風。前提不成立（吵雜背景、遠場）時能量門檻會誤判，屆時再考慮換函式庫（列為 Risk 的 open question），本次以旗標與可調門檻涵蓋常見情境即可。

### 串流式擷取取代固定秒數睡眠

把 `record_pcm16` 改成：開串流後進入輪詢迴圈，每隔一個窗長從共享緩衝 `drain` 新樣本，下採樣／降至單聲道後算該窗 RMS，餵入 VAD 狀態機；狀態機回報 `Stop` 時結束並回傳累積的 16kHz mono `i16`。保留最長時限（`FLEETY_VAD_MAX_MS`）與開口前逾時（`FLEETY_VAD_START_TIMEOUT_MS`）兩個硬上限。`FLEETY_VAD=off` 時走原本的固定 `sleep(FLEETY_STT_SECONDS)` 分支，維持回溯相容。`record_wav` 與 `capture_audio` 不需改介面，只因底層擷取換掉而受益。

### Barge-in：TTS 子行程 spawn + 麥克風 onset 監看

新增 `speak_interruptible(text) -> SpeakOutcome`（`Completed` / `Interrupted` / `Unavailable`）。實作：以 `Command::spawn()` 起 TTS 子行程取得 `Child` handle；同時開麥克風串流，用 onset 判定（連續 N 個窗能量高於門檻才算開口，抑制回音與瞬時雜訊）監看；偵測到 onset 就 `child.kill()` 並回 `Interrupted`，否則 `child.wait()` 到播完回 `Completed`。`FLEETY_BARGE_IN=off`、無麥克風或無引擎時，退化為等同現行阻塞播放（`Unavailable`/`Completed`），不 crash。`main.rs` 把 `voice::speak(&spoken)` 換成 `speak_interruptible`；因收訊迴圈在 `Done` 後本就回到外層去擷取輸入，`Interrupted` 只需讓 `speak` 及早返回即可，主迴圈結構幾乎不動。onset 之後的擷取沿用新的 VAD 擷取（其開口前逾時足以涵蓋短暫接續），第一個字可能被輕微裁掉，列為可接受的 Risk。

### 純函式化 VAD 狀態機以利測試

把可測邏輯抽離硬體：`rms(&[f32]) -> f32`；`VadConfig`（由 env 解析）；`VadState`（欄位含目前階段、累積靜音時間、累積總時間）其 `observe(window_rms, window_ms) -> VadDecision`（`WaitingForSpeech` / `Speaking` / `Stop(EndReason)`，`EndReason` 為 `Silence` / `MaxDuration` / `StartTimeout`）；以及 onset 判定 `fn onset_reached(consecutive_hot: u32, needed: u32) -> bool`。cpal 擷取迴圈與 `speak_interruptible` 只做 I/O 編排，把每個窗的數據餵給這些純函式。單元測試餵合成序列驗證端點行為，不需麥克風。

### 設定旗標與回溯相容

新增 env：`FLEETY_VAD`(on/off，預設 on)、`FLEETY_VAD_ENERGY`(RMS 門檻，預設保守值如 0.02)、`FLEETY_VAD_SILENCE_MS`(尾端靜音 hangover，預設 ~800)、`FLEETY_VAD_MAX_MS`(最長 utterance，預設 ~15000)、`FLEETY_VAD_START_TIMEOUT_MS`(開口前逾時，預設 ~8000)、`FLEETY_BARGE_IN`(on/off，預設 on)。全部無效值退回預設（延續 voice.rs 既有解析慣例，如 `stt_seconds`）。`FLEETY_STT_SECONDS` 在 `FLEETY_VAD=off` 時續用，舊使用者行為不變。

## Implementation Contract

- 行為：
  - VAD 擷取 — 一開始即聆聽；`observe` 在偵測到首個高能量窗前回 `WaitingForSpeech`，累積靜音達 `silence_ms` 回 `Stop(Silence)`，總時長達 `max_ms` 回 `Stop(MaxDuration)`，未開口而總時長達 `start_timeout_ms` 回 `Stop(StartTimeout)`。擷取函式在 `Stop(StartTimeout)` 且從未偵測到語音時回傳「無擷取」（`None`／空），其餘情況回傳累積樣本。
  - Barge-in — `speak_interruptible` 在 `FLEETY_BARGE_IN=on` 且麥克風可用時，onset 達標即 kill TTS 子行程回 `Interrupted`；否則播到完回 `Completed`；引擎不可用回 `Unavailable`。
- 介面/資料形狀：
  - `fn rms(samples: &[f32]) -> f32`
  - `struct VadConfig { energy: f32, silence_ms: u64, max_ms: u64, start_timeout_ms: u64 }` + `fn vad_config_from_env() -> VadConfig`
  - `enum VadDecision { WaitingForSpeech, Speaking, Stop(EndReason) }`、`enum EndReason { Silence, MaxDuration, StartTimeout }`
  - `struct VadState`，`fn observe(&mut self, window_rms: f32, window_ms: u64) -> VadDecision`
  - `fn onset_reached(consecutive_hot: u32, needed: u32) -> bool`
  - `enum SpeakOutcome { Completed, Interrupted, Unavailable }`、`fn speak_interruptible(text: &str) -> SpeakOutcome`
  - `record_pcm16` 簽章不變（仍回 `Option<Vec<i16>>`），內部改 VAD 驅動；`FLEETY_VAD=off` 走固定秒數。
- 失敗模式：無輸入裝置、不支援格式、串流建立失敗、子行程 spawn/kill 失敗 → 分別退化為「無擷取」或「等同阻塞播放」，一律回 `None`/`Unavailable`/`false` 而非 panic，符合 `#![warn(clippy::unwrap_used, clippy::expect_used)]`。
- 驗收條件：
  - 純函式測試涵蓋 silence 端點、max-duration、start-timeout、onset 需連續多窗、RMS 對靜音與滿幅的量級關係。
  - `FLEETY_VAD=off` 時行為與現行固定秒數一致（既有測試不回歸）。
  - `main.rs` 語音迴圈以 `speak_interruptible` 取代 `speak`，仍能顯示文字並播放；barge-in 開關可切換。
  - `cargo test -p fleety-cli`、`cargo clippy -p fleety-cli` 通過。
- 範圍邊界：只改 `voice.rs` 與 `main.rs`；不新增依賴、不改 protocol、不改其他 crate。

## Risks

- 聲學回音誤觸發：喇叭 TTS 被麥克風收到可能被當成 barge-in。緩解：onset 需連續多窗、門檻可調、可 `FLEETY_BARGE_IN=off`。不做 AEC。
- 門檻調校：預設 `FLEETY_VAD_ENERGY` 在不同麥克風增益下可能過鬆或過嚴，導致提早斷句或錄不到。緩解：全部門檻可由 env 覆寫，並保留 `FLEETY_VAD=off` 回退。
- Barge-in 後首字裁切：停播到新擷取之間可能漏掉開頭一兩個字。本次接受，未來可讓監看與擷取共用同一串流以保留 onset 音訊（open question）。
- 平台差異：kill TTS 子行程在三個平台行為需各自驗證（PowerShell 子行程樹、`spd-say --wait`、`say`）。若某平台 kill 不即時，退化為播完，不 crash。
- Open question：能量門檻在吵雜／遠場環境不足時是否引入 `webrtc-vad`/Silero — 屬後續變更，需產品拍板是否接受新依賴。