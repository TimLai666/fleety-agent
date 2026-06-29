## 1. 重試核心(純函式)

- [x] 1.1 在 crates/agent-core/src/retry.rs 實作錯誤分類 `classify`(可重試 vs 不可重試)與 `RetryConfig::from_env`(重試上限與基底/上限,0=關閉),交付 "Transient model-call failures are retried with backoff" 的分類面與 "Retries are bounded and configurable" 的設定面;對應設計「錯誤分類:可重試 vs 不可重試」與「純函式化退避與分類以利測試」。先寫失敗測試:429/408/425/500/502/503/504 與連線逾時 → Retry,400/401/403/404 → Fatal;`from_env` 的預設值與「0 關閉」解析。
- [x] 1.2 實作 `backoff_delay`(指數成長、夾在 cap、full jitter 由參數注入、`Retry-After` 秒數覆寫),交付 "Transient model-call failures are retried with backoff" 的退避與 Retry-After 面;對應設計「指數退避含 jitter,並尊重 Retry-After」。先寫失敗測試:指數成長且夾在 cap、Retry-After 覆寫計算值、注入的 jitter 可重現(用 spec example 的分類/數值)。

## 2. 重試驅動器

- [x] 2.1 實作重試驅動器:對「單次嘗試」閉包重試,依 `classify` 決定重試或快速失敗,依 `backoff_delay` 以 tokio sleep 等待,耗盡時回 `CoreError::Provider`(訊息標明已重試 N 次)且絕不 panic,交付 "Non-retryable failures fail fast" 與 "Retries are bounded and configurable" 的耗盡面;對應設計「重試上限與可設定參數」。驗證:以可注入的假嘗試序列(成功前先回 N 次可重試錯誤、或回不可重試錯誤)測試重試成功、快速失敗、耗盡三條路徑。

## 3. 接上 provider

- [x] 3.1 [P] 在 crates/agent-core/src/openai.rs 的 `complete` 與 `complete_streaming` 以重試驅動器包住單次嘗試,串流僅在尚未吐出任何 delta 之前重試,交付 "Streaming retries only before output begins"(OpenAI 路徑);對應設計「串流與非串流路徑都包上重試」。驗證:既有 openai provider 測試全綠;串流已吐字後失敗不重啟(假串流序列或程式碼審查確認)。
- [x] 3.2 [P] 在 crates/agent-core/src/gemini.rs 的 `complete` 與 `complete_streaming` 同樣包上重試並套用相同串流規則,交付 "Streaming retries only before output begins"(Gemini 路徑);對應設計「串流與非串流路徑都包上重試」。驗證:既有 gemini provider 測試全綠。

## 4. 設定與文件

- [x] 4.1 [P] 把 `FLEETY_MODEL_RETRIES` / `FLEETY_MODEL_RETRY_BASE_MS` / `FLEETY_MODEL_RETRY_CAP_MS` 登記到 typed config registry(crates/fleety-tools/src/config.rs)並更新 docs/env.md,交付 "Retries are bounded and configurable" 的可設定面。驗證:`config list` 顯示這些鍵;內容審查 docs/env.md 涵蓋預設與「0 關閉」語意。
