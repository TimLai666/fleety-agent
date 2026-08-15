## 1. 核心政策與閘門介面

- [x] [P] 1.1 實作 `Policy::AutoReview` 與帶有 objective、bounded conversation context、sanitized arguments、risk、danger signals 的 review context，讓「Use a third policy at the existing gate」與「Full access by default」的 read/mutate/critical 行為可區分，並以 agent-core 單元測試驗證三種政策的 gating 結果。
- [x] [P] 1.2 擴充設定 registry、server/shared config surfaces 與驗證器，讓「Access policy and authentication」接受 `auto_review` 及正整數 `FLEETY_AUTO_REVIEW_TIMEOUT_SECS`，並以 config registry 與 invalid-value 測試驗證預設值、列舉值及 timeout fallback。

## 2. 便宜模型審核器

- [x] 2.1 實作「Build one server-side auto-review gate on the cheap provider」，使用 `ProviderTiers::resolve("cheap")` 發送無工具規格的非串流 review call，解析嚴格 `{"decision":"approve|deny","reason":"..."}`，並以 unit tests 驗證 approve、deny、timeout、provider error、invalid JSON、tool-call response 與 oversized response 全部符合「Auto review uses a strict fail-closed decision」。
- [x] [P] 2.2 實作「Preserve deterministic detectors as trusted warning signals」與「Auto review receives objective and trusted danger signals」，建立 bounded prompt sections、prompt-injection 邊界、danger-signal code/message 與 secret/path redaction，並以 prompt snapshot、redaction 與 dangerous-command signal tests 驗證「Auto reviewer is toolless and secrets are protected」。
- [x] 2.3 將 AutoReviewGate 接到共用 agent turn gate，落實 Implementation Contract 的 behavior 與 interface / data shape，讓「Auto review gates unattended tool execution」涵蓋 workspace、device-routed、remote、subagent、scheduled、WebSocket、SSE 及 recovery paths，並以各 execution path integration tests 驗證 auto_review 不產生 human approval frame 且只有 approve 才呼叫 candidate tool。

## 3. 危險操作與檔案邊界

- [x] [P] 3.1 更新 `run_command` 的「Run shell commands with a critical-command guard」，在 `auto_review` 將 `rm -rf /`、`mkfs`、`dd`、disk wipe、shutdown/reboot 等偵測結果轉成 reviewer danger signals，並以 full_access、require_approval、auto_review 三組 tests 驗證拒絕、人工 gate 與模型裁決的差異。
- [x] [P] 3.2 更新檔案工具的「Filesystem scope and sensitive-path guard」，讓 `auto_review` 對 SSH keys/config、`/etc/shadow`、`/dev`、Windows system directories 等 mutation 產生 trusted sensitive-path signals，同時維持 workspace scope 逃逸拒絕，並以 path-boundary tests 驗證 approve、deny 與 scope rejection。

## 4. 審計與錯誤處理

- [x] [P] 4.1 實作「Audit decisions without exposing candidate secrets」與「Auto review records an auditable outcome」，記錄 decision、risk、tool、provider/model label、danger codes、latency、sanitized reason 及 review failure category，並以「List recent audit entries」測試驗證 denied/not-executed 狀態可查且 raw arguments、prompt、tokens、API keys、passwords 不會出現。
- [x] [P] 4.2 實作「Make all auto-review failures deny without human fallback」，覆蓋 Implementation Contract 的 failure modes，把 missing cheap provider、timeout、retry exhaustion、missing context、redaction failure、invalid response 與 protocol violation 統一轉為 synthetic denial result，並以 failure-injection tests 驗證 candidate tool 永不執行且不發出 interactive approval request。

## 5. 設定、文件與完整驗證

- [x] [P] 5.1 更新 `docs/env.md`、`docs/tools.md`、`README.md` 與相關 spec references，說明 `auto_review`、cheap provider、timeout、danger warnings、fail-closed 與 no-human-approval 行為，並以文件內容檢查與 `spectra analyze auto-review --json` 驗證沒有過時的二元政策描述。
- [x] 5.2 執行完整變更驗證，依 Implementation Contract 的 acceptance criteria 與 scope boundaries 確認既有 `full_access` 與 `require_approval` 行為不回歸，確認所有「Auto review gates unattended tool execution」執行面都有 shared gate 覆蓋，並執行 agent-core、fleety-tools、fleety-server、fleety-cli、fleetyd 相關測試與 `spectra validate auto-review`。
