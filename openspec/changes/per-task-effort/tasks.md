## 1. Effort 型別與 provider 翻譯(純函式 + 簽章)

- [x] 1.1 在 crates/agent-core/src/model.rs 定義 `enum Effort { Low, Medium, High }`(含 from_str/as_str),並在 `ModelProvider::complete` / `complete_streaming` 增加附加參數 `effort: Option<Effort>`(更新所有 impl 與呼叫端,None 維持相容),交付 "Model calls carry an optional reasoning effort" 的型別與貫穿面;對應設計「Effort 型別與可選地貫穿模型呼叫」。先寫失敗測試:Effort 字串解析(low/medium/high/未知→None)。
- [x] 1.2 實作 (scheme, effort)→請求欄位 的純函式(OpenAiReasoning→reasoning_effort、GeminiThinking→thinking、None→不產欄位),交付 "Model calls carry an optional reasoning effort" 的翻譯/省略面;對應設計「provider 依家族翻譯,不支援則省略」。先寫失敗測試:用 spec example 表逐列驗證輸出(含 None→無欄位)。

## 2. 接上 provider

- [x] 2.1 [P] 在 crates/agent-core/src/openai.rs 讓 provider 帶 effort_scheme,送請求時依 task 1.2 的純函式加入或省略 reasoning_effort,交付 "Model calls carry an optional reasoning effort"(OpenAI 路徑)。先寫失敗測試:scheme=OpenAiReasoning + effort=high → body 含 reasoning_effort:"high";scheme=None → body 無該欄位。
- [x] 2.2 [P] 在 crates/agent-core/src/gemini.rs 讓 provider 帶 effort_scheme,送請求時依純函式加入或省略 thinking 設定,交付 "Model calls carry an optional reasoning effort"(Gemini 路徑)。先寫失敗測試:scheme=GeminiThinking + effort → body 含 thinking 設定;scheme=None → 無。

## 3. 主 agent 自調 + subagent 由 parent 指定

- [x] 3.1 在對話/session 層加入 effort 狀態並新增 set_effort 工具(於 crates/fleety-server 註冊),turn loop 每回合把 session effort 當作 effort 傳給模型呼叫,交付 "The main agent sets its own effort dynamically";對應設計「主 agent 動態自調自己的 effort(session 狀態 + 工具)」。驗證:呼叫 set_effort(high) 後,下一次 turn 傳入的 effort 為 high(單元/整合測試)。
- [x] 3.2 在 crates/agent-core/src/subagent.rs 的 `SpawnRequest` 加 `effort: Option<Effort>`,spawn 工具接受 `"effort"` 參數由 parent 指定,未指定則用預設;subagent 各回合用該固定 effort 且無自調途徑,交付 "A subagent's effort is decided by the spawning agent";對應設計「subagent 的 effort 由 parent 在 spawn 時決定」。先寫失敗測試:spawn 帶 effort=low → subagent 呼叫帶 low;未帶 → 用預設。

## 4. 預設設定 + 文件

- [x] 4.1 [P] 把 `FLEETY_MODEL_EFFORT` / `FLEETY_CHEAP_MODEL_EFFORT` 登記到 typed config registry,並在 providers.rs 依模型名稱/設定決定 effort_scheme 與注入預設 effort;更新 docs/env.md,交付 "Default effort is configurable per tier";對應設計「預設 effort 可設定」。驗證:`config list` 顯示這兩鍵;未設定時模型呼叫不帶 effort 欄位(相容);docs/env.md 說明 low/medium/high 與未設定語意。
