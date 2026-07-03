## 1. Auth: account id capture

- [x] 1.1 [P] 在 crates/fleety-tools/src/oauth.rs 加純函式 `parse_account_id(id_token) -> Option<String>`:切 JWT、base64url 解 payload、讀 `chatgpt_account_id`,fallback 到 `["https://api.openai.com/auth"].chatgpt_account_id` 再到第一個 `organizations[].id`;壞 token 回 None 不報錯。驗證:有效 JWT 取到 id、fallback claim、garbage→None 的單元測試(用自組的 base64url payload)。(涵蓋需求: The ChatGPT account id is captured at login)
- [x] 1.2 `Tokens` 加 `account_id: Option<String>`(serde default);`exchange_code` 從回應的 `id_token` 解出 account_id 存入,`refresh_access_token` 無 id_token 時保留舊值。驗證:exchange stub 回含 id_token → Tokens.account_id 有值;refresh stub 無 id_token → 沿用傳入舊 account_id 的單元測試。(涵蓋需求: The ChatGPT account id is captured at login)

## 2. agent-core: CodexAuth 抽象與 Responses provider

- [x] 2.1 在 agent-core 定義 `CodexAuth` trait(`async fn credentials() -> Result<CodexCreds>`)與 `CodexCreds { bearer, account_id }`,從 lib.rs 匯出。驗證:編譯 + 一個 stub 實作可被呼叫的測試。(涵蓋需求: The oauth:codex auth mode builds the Responses provider)
- [x] 2.2 建立 crates/agent-core/src/codex_responses.rs 的 `CodexResponses` 骨架(base_url/model/Arc<dyn CodexAuth>/client/caps/effort + new/with_capabilities/with_effort_config),impl `ModelProvider`;`capabilities()`/`with_effort()` 比照 OpenAiCompat。驗證:建構 + capabilities 回傳注入值的單元測試。(涵蓋需求: A Codex Responses provider calls the model over the Responses API)
- [x] 2.3 實作純函式 `build_request_body(model, messages, tools) -> Value`:system→instructions、user/assistant 文字→message input item、assistant tool_calls→function_call item、tool 結果→function_call_output item、tools→function tool;固定欄位 tool_choice/parallel_tool_calls/store/stream/include。驗證:對照 design D3 形狀的單元測試(含一輪 tool call + tool result 的歷史映射)。(涵蓋需求: Tool calls work over the Responses API)
- [x] 2.4 實作純函式 `assemble_responses_sse(body) -> ModelResponse`:累積 `response.output_text.delta`、`response.output_item.done` 的 function_call → ToolCall、`response.completed`/`[DONE]` 結束;壞行跳過。驗證:文字 delta + 一個 function_call item → 含 content 與 tool call 的 ModelResponse、壞行被略過的單元測試。(涵蓋需求: The provider parses the Responses SSE stream)

## 3. Provider 請求路徑與 headers

- [x] 3.1 實作 `CodexResponses::complete_streaming`:取 credentials(未登入回可行動錯誤)、POST backend URL、設全部 Codex headers(Authorization/chatgpt-account-id/OpenAI-Beta/originator/session_id/Accept/User-Agent;account_id 缺則略過該 header)、經 retry 讀 SSE、逐 delta 呼叫 on_delta;`complete` 走同路徑但 delta sink 為 no-op。驗證:對 stub server 發一次請求,捕捉並斷言 headers 與 body 形狀、回組裝訊息的整合測試(SSE 罐頭回應)。(涵蓋需求: A Codex Responses provider calls the model over the Responses API;The provider parses the Responses SSE stream)
- [x] 3.2 未登入路徑:credentials() 回錯誤時 complete/complete_streaming 直接回該可行動錯誤、不 POST。驗證:auth stub 回 not-logged-in → complete() 回含「auth login」訊息、無 HTTP 呼叫的單元測試。(涵蓋需求: The oauth:codex auth mode builds the Responses provider)

## 4. 接線與文件

- [x] 4.1 在 crates/fleety-tools/src/oauth.rs 加 `OAuthCodexAuth` 實作 `agent_core::CodexAuth`:讀 token store、依 plan_bearer refresh、回 (access_token, account_id)。驗證:已登入 stub → 回 bearer+account_id;未登入 → 可行動錯誤的單元測試。(涵蓋需求: The oauth:codex auth mode builds the Responses provider)
- [x] 4.2 在 crates/fleety-server/src/providers.rs 的 build_provider:`auth_is_oauth` 為真時改建 `CodexResponses`(base=FLEETY_CODEX_BACKEND_URL、model、auth=OAuthCodexAuth)取代 OpenAiCompat+bearer;非 oauth 路徑不變。驗證:`FLEETY_MODEL_AUTH=oauth:codex` → build 出的 provider 未登入時 complete 回可行動錯誤;無 auth → 仍是 OpenAiCompat 行為的測試。(涵蓋需求: The oauth:codex auth mode builds the Responses provider)
- [x] 4.3 [P] 更新 docs/env.md 的 Codex 段:移除「model 呼叫還接不上」的 caveat,改述 oauth:codex 現在經 Responses API 端到端可用(含 originator 預設 codex_cli_rs、Responses 端點),並註明實機待驗。驗證:內容審查對照實作。
