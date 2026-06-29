## Why

模型呼叫目前是「單次、無重試」:`openai.rs` 與 `gemini.rs` 對所有非 2xx 回應一視同仁地立即回錯(各自的 status 檢查處),沒有任何退避重試。實務上模型端點常出現可恢復的暫時性失敗——429(rate limit / quota)、5xx、連線重置、逾時——這些目前都會直接讓一整個 turn 失敗,使用者得手動重試。daemon 雖有指數退避,但只用於 WebSocket 重連,完全不涵蓋模型呼叫。

## What Changes

- 在 provider 層(agent-core)為模型呼叫加入**依錯誤類型的重試**:429 與 5xx、連線/逾時等暫時性錯誤用**指數退避(含 jitter)**重試;遇 `Retry-After` header 時以其指定秒數為準。
- 4xx(認證、請求格式等不可重試錯誤)**快速失敗**,不浪費重試。
- 重試次數有上限;基底退避、上限可由 `FLEETY_*` 設定調整。
- 重試耗盡或遇不可重試錯誤時,沿用既有 errors-as-message(`CoreError::Provider` + remediation)回報,**絕不 panic**。
- non-streaming(complete)與 streaming(complete_streaming)兩條路徑都涵蓋。

## Non-Goals

(本變更會建立 design.md,Non-Goals 寫在 design 的 Goals/Non-Goals 一節。)

## Capabilities

### New Capabilities

- `resilient-model-calls`: 模型呼叫的韌性層——錯誤分類(可重試 vs 不可重試)、指數退避含 jitter、尊重 Retry-After、重試上限與可設定參數、耗盡後以訊息回報不崩潰;涵蓋串流與非串流路徑。

### Modified Capabilities

(none)

## Impact

- Affected specs: resilient-model-calls(新)
- Affected code:
  - Modified:
    - crates/agent-core/src/openai.rs(complete / complete_streaming 包上重試)
    - crates/agent-core/src/gemini.rs(同上)
  - New:
    - crates/agent-core/src/retry.rs(錯誤分類 + 退避時程的純函式 + 重試驅動器)
  - 可能 Modified:
    - crates/fleety-tools/src/config.rs(新增重試相關 FLEETY_* 設定到 registry)
    - docs/env.md(記錄新設定)
