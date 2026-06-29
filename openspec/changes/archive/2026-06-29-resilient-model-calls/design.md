## Context

模型呼叫在 agent-core 的兩個 provider(OpenAI 相容、Gemini)裡各自送一次 HTTP 請求,非 2xx 一律立即轉成 `CoreError::Provider` 回報,無重試。錯誤經 `error.rs` 帶 remediation 往上拋給 turn loop。daemon 的指數退避(backoff.rs)只服務 WebSocket 重連,與模型呼叫無關。兩個 provider 各有 non-streaming(complete)與 streaming(complete_streaming)兩條路徑。約束:agent-core 不得依賴任何 fleety crate;forbid unsafe;never-crash;env 測試可單執行緒。

## Goals / Non-Goals

**Goals:**

- 暫時性失敗(429、5xx、連線/逾時)能自動以指數退避重試,提升一個 turn 成功完成的機率。
- 尊重伺服器的 `Retry-After`(秒數或 HTTP-date)作為等待依據。
- 不可重試錯誤(4xx 認證/請求錯誤)快速失敗,不空轉。
- 重試耗盡仍以既有 errors-as-message 回報,絕不 panic。
- 重試行為的「分類」與「退避時程」為純函式,可單元測試;HTTP 往返維持手動驗證。

**Non-Goals:**

- 不做主模型 429 時自動 fallback 到 cheap tier(列為 Open Question,屬另一變更)。
- 不改 `ModelProvider` trait 的對外語意或方法簽章。
- 不改 wire 協定、不引入新的重型依賴(用既有 reqwest + tokio sleep)。
- 不處理「整個 turn 層級」的重試(只在單次模型呼叫層級)。

## Decisions

### 錯誤分類:可重試 vs 不可重試

把 HTTP 結果分成三類:可重試(429、408、425、500、502、503、504,以及 reqwest 的連線/逾時類錯誤)、不可重試(其餘 4xx,如 400/401/403/404)、成功。分類做成純函式 `classify(status: Option<u16>, is_timeout_or_connect: bool) -> Retryable`,不接觸網路,便於窮舉測試。理由:429/5xx/連線問題通常可恢復;4xx 是請求本身的問題,重試無意義且浪費額度。

### 指數退避含 jitter,並尊重 Retry-After

第 n 次重試的基礎等待 = `base * 2^(n-1)`,夾在 `[base, cap]`,再加上 full jitter(0..delay 的隨機量)。若回應帶 `Retry-After`(秒數或 HTTP-date),以其值取代計算值(仍夾在 cap 內)。退避時程做成純函式 `backoff_delay(attempt, base, cap, retry_after, rand_unit) -> Duration`(隨機與時間由參數注入),可測。理由:jitter 避免重試風暴;Retry-After 是伺服器的明確指示,優先採用。

### 重試上限與可設定參數

預設最多重試 N 次(預設 3),基底退避、上限可由 `FLEETY_*` 設定調整(例如 `FLEETY_MODEL_RETRIES`、`FLEETY_MODEL_RETRY_BASE_MS`、`FLEETY_MODEL_RETRY_CAP_MS`),並登記到 typed config registry。理由:不同端點容忍度不同;0 次重試應可關閉(退回現行單次行為)。

### 串流與非串流路徑都包上重試

把重試驅動器寫成一個包裝:對「建立請求→送出→判定結果」的閉包做重試。non-streaming 直接包整個請求。streaming 只在**尚未開始送出任何 delta 之前**的失敗才重試(連線/初始 HTTP 狀態);一旦已開始串流則不重試(避免重複輸出),改為以錯誤結束該次。理由:串流一旦吐出 token 就無法乾淨重來。

### 純函式化退避與分類以利測試

`classify` 與 `backoff_delay` 不依賴時鐘或亂數來源(由參數注入),放在新檔 `crates/agent-core/src/retry.rs`,搭配 `RetryConfig`(從 env 讀取的設定快照)。重試驅動器負責 `tokio::time::sleep`。理由:把難測的 I/O/時間隔離在薄殼,核心邏輯可窮舉測試。

## Implementation Contract

**行為(Behavior):**

- 模型呼叫遇 429/5xx/連線/逾時:依退避時程重試,至多 N 次;遇 `Retry-After` 以其秒數等待。
- 遇 4xx(非 429)不可重試錯誤:立即回 `CoreError::Provider`,不重試。
- 重試全部失敗:回 `CoreError::Provider`,訊息含最後一次的 status/原因 + 既有 remediation;永不 panic。
- 串流:已開始吐 delta 後的失敗不重試,以錯誤結束。
- `FLEETY_MODEL_RETRIES=0` 時行為等同現行單次請求。

**介面 / 資料形狀:**

- 新檔 `crates/agent-core/src/retry.rs` 匯出:`enum Retryable { Retry, Fatal }`(或等義)、`fn classify(status: Option<u16>, transient: bool) -> Retryable`、`fn backoff_delay(attempt, base, cap, retry_after, jitter_unit) -> Duration`、`struct RetryConfig { retries, base, cap }` 與其 `from_env()`。
- provider 的 `complete` / `complete_streaming` 內部以重試驅動器包住單次嘗試;對外簽章不變。
- 新 `FLEETY_*` 設定鍵登記到 config registry(scope:Server 或 Shared)。

**失敗模式:**

- 不可重試錯誤:立即 `CoreError::Provider`(快速失敗)。
- 可重試但耗盡:`CoreError::Provider`,訊息標明已重試 N 次。
- Retry-After 無法解析:退回計算的退避值。

**驗收標準(Acceptance):**

- 單元測試:`classify` 對 429/408/500/502/503/504/連線逾時 → Retry;400/401/403/404 → Fatal。
- 單元測試:`backoff_delay` 指數成長、夾在 cap、Retry-After 覆寫、jitter 注入可重現。
- 單元測試:`RetryConfig::from_env` 解析(含預設與 0 次關閉)。
- 既有 provider 測試全綠(行為相容,預設仍能完成一次成功呼叫)。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒可跑。
- HTTP 實際重試往返為手動驗證(可選:以可注入的假回應序列測試重試驅動器)。

**範圍邊界:**

- In scope:provider 層單次呼叫的重試/退避/分類、設定鍵、串流的「未吐字前才重試」規則、文件。
- Out of scope:turn 層級重試、429→cheap tier fallback、trait 簽章變更、wire 協定變更。

## Risks / Trade-offs

- [串流已吐字後重試會造成重複輸出] → 規則明訂:已開始串流不重試,以錯誤結束。
- [重試放大對端點的壓力] → jitter + 上限 + 尊重 Retry-After;預設次數保守(3)。
- [Retry-After 為 HTTP-date 格式解析複雜] → 先支援秒數;date 格式解析失敗則退回計算值(標為可接受)。
- [亂數/時鐘讓測試不穩] → 由參數注入,純函式測試。

## Migration Plan

- 純加層:不改 trait 與 wire。預設次數>0 即啟用;設 `FLEETY_MODEL_RETRIES=0` 立即退回現行單次行為(等同回滾)。
- 無資料遷移。

## Open Questions

- 主模型 429 耗盡時是否自動降級到 cheap tier:本變更不做,留待「能力感知模態 / tier 路由」相關變更一併考量。
- `Retry-After` 的 HTTP-date 形式是否需要完整支援:初版只做秒數。
