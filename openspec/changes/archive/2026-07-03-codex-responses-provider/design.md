## Context

`codex-oauth` (archived) added ChatGPT login, a 0600 token store, refresh, and a `BearerSource` trait that `OpenAiCompat` consults. But Codex's model backend is the **Responses API** (`https://chatgpt.com/backend-api/codex/responses`), whose request/response shape differs from `/chat/completions`. So `auth = "oauth:codex"` authenticates but can't call the model. The exact Responses contract (endpoint, headers, request fields, SSE event types) is the one the upstream Codex CLI uses, mirrored by `codex-openai-proxy` (Rust) and `heddle`; this design follows that verified contract. `agent-core` already has the `ModelProvider` trait, `Message`/`ToolCall`/`ToolSpec` types, and an SSE read loop pattern (byte stream to `data:` lines) in `openai.rs`.

## Goals / Non-Goals

**Goals:**

- A `CodexResponses` provider in `agent-core` implementing `ModelProvider` over the Responses API, with the Codex headers and SSE parsing.
- Tool-calling: map `ToolSpec`/history to Responses `tools` plus `function_call`/`function_call_output` input items, and recover `function_call` outputs.
- Capture the ChatGPT `account_id` from the OAuth `id_token` at login and expose bearer plus account id to the provider.
- `auth = "oauth:codex"` builds `CodexResponses` (pointed at the Codex backend) instead of `OpenAiCompat`.

**Non-Goals:**

- General/API-key Responses usage (`api.openai.com/v1/responses`); non-Codex accounts.
- Reasoning-item surfacing, multimodal Responses parts (image/audio) — text plus tool calls only for now.
- Live verification against the real Codex backend (network-gated; shapes are unit-tested offline).

## Decisions

**D1 New provider, mirroring `OpenAiCompat`'s structure.** `CodexResponses` holds base URL, model, an auth source, a reqwest client, caps, and effort. It implements `ModelProvider`; the Responses backend streams (SSE), so `complete()` runs the streaming path with a no-op delta sink and `complete_streaming()` emits deltas. Reuses `crate::retry` for the initial connection/status. Rejected: extending `OpenAiCompat` with a mode flag — the request/response shapes are different enough that a separate type is clearer.

**D2 Auth via a `CodexAuth` trait in `agent-core`.** `trait CodexAuth { async fn credentials() -> Result<CodexCreds> }` returning `CodexCreds { bearer: String, account_id: Option<String> }`. The provider calls it once per request (an OAuth impl may refresh). The impl lives in `fleety-tools` (reads the token store, refreshes) so `agent-core` keeps zero Fleety deps. Rejected: reusing `BearerSource` (it returns only a token, not the account id).

**D3 Request mapping (Responses API).** Body: `{ model, instructions, input, tools, tool_choice:"auto", parallel_tool_calls:false, store:false, stream:true, include:[] }`.
- System messages concatenate into `instructions` (a default instruction when none).
- User/assistant text becomes `{ type:"message", role, content:[{ type:"input_text", text }] }`.
- Assistant `tool_calls` become one `{ type:"function_call", name, arguments:<stringified JSON>, call_id }` per call.
- `Role::Tool` result becomes `{ type:"function_call_output", call_id:<tool_call_id>, output:<content> }`.
- `tools` become `{ type:"function", name, description, parameters:<JSON Schema>, strict:false }`.

**D4 Headers (verified from codex-openai-proxy).** `Authorization: Bearer <access_token>`, `chatgpt-account-id: <account_id>`, `OpenAI-Beta: responses=experimental`, `originator: <FLEETY_CODEX_ORIGINATOR default codex_cli_rs>`, `session_id: <uuid v4>`, `Content-Type: application/json`, `Accept: text/event-stream`, plus a `User-Agent`. A missing `account_id` omits that header (surfaced as a likely-login error upstream).

**D5 SSE parsing.** Read the byte stream line-by-line like `openai.rs`; each `data:` line is JSON with a `type`. Accumulate `response.output_text.delta` (`delta` appended plus `on_delta`); on `response.output_item.done` where `item.type=="function_call"` collect a `ToolCall { id:call_id, name, arguments:parse(arguments) }`; finish on `response.completed` or `[DONE]`. Build `ModelResponse { Message::assistant(text) with tool_calls }`.

**D6 Capture `account_id` at login.** Add `parse_account_id(id_token) -> Option<String>`: split the JWT on `.`, base64url-decode the payload, read `chatgpt_account_id`, falling back to `["https://api.openai.com/auth"].chatgpt_account_id` then the first `organizations[].id`. `Tokens` gains `account_id: Option<String>`. `exchange_code` decodes it from the returned `id_token`; `refresh_access_token` keeps the previous `account_id` when the refresh response omits an `id_token`.

**D7 Wiring.** In `providers.rs`, when `auth_is_oauth(auth)`, build a `CodexResponses` (base = `FLEETY_CODEX_BACKEND_URL`, model, auth = an `OAuthCodexAuth` over the token store) instead of `OpenAiCompat`. Non-oauth paths are unchanged.

## Implementation Contract

**Behavior:** After `fleety auth login`, setting a provider to `auth = "oauth:codex"` lets the agent run turns against the ChatGPT/Codex backend: assistant text streams back and tool calls drive the tool loop, using the account's OAuth token (auto-refreshed) and account id — no API key.

**Interface / data shape:**
- `agent_core::CodexResponses` implementing `ModelProvider`; constructor `CodexResponses::new(base_url, model, Arc<dyn CodexAuth>)` plus `with_capabilities`/`with_effort_config`.
- `agent_core::CodexAuth` trait: `async fn credentials() -> Result<CodexCreds>`; `CodexCreds { bearer: String, account_id: Option<String> }`.
- Request body and headers exactly as D3/D4.
- `fleety_tools::oauth::Tokens` gains `account_id: Option<String>`; `parse_account_id(&str) -> Option<String>`; an `OAuthCodexAuth` implementing `CodexAuth`.
- SSE handled event types: `response.output_text.delta`, `response.output_item.done` (function_call), `response.completed`, `[DONE]`.

**Failure modes:** No stored tokens gives `credentials()` the actionable "run fleety auth login" error, propagated by `complete()`. HTTP non-2xx (before any delta) is retried per `retry` then an actionable provider error. Missing `account_id` omits the header (Codex will reject, error surfaced, not a crash). Malformed SSE lines are skipped. Never panics.

**Acceptance criteria:**
- Unit tests: request-body mapping (messages/tool history to `input`, tools to function tools, instructions from system), pure `parse_account_id` (valid JWT to id; fallback claim; garbage to None), SSE assembly (text deltas plus a `function_call` item to a `ModelResponse` with content and tool call), header set (via a stub server capturing the request), and `credentials()` not-logged-in to an actionable error.
- Integration (stub server): a full `complete()` against a canned SSE body returns the assembled message; `auth = "oauth:codex"` builds a `CodexResponses` (asserted by behavior — it targets the Codex backend and errors actionably when logged out).
- Existing provider behavior and tests unchanged.

**Scope boundaries:** In scope = the Responses provider (text plus tool-calling plus SSE), account-id capture at login, and the `oauth:codex` wiring. Out of scope = API-key Responses, non-Codex accounts, reasoning/multimodal Responses parts, live-backend verification.

## Risks / Trade-offs

- [Codex backend shape/headers drift or anti-automation] mitigated by following the documented/mirrored contract exactly; headers, `originator`, and endpoint are overridable via config; failures are actionable, never crashes. Live verification is a known follow-up (like SSH/CDP).
- [Responses tool-call/history mapping subtly wrong] mitigated by request-body unit tests against the documented shape; the mapping is a pure function for testability.
- [`account_id` claim location varies] mitigated by a three-tier fallback in `parse_account_id`, matching heddle; omit the header if none and surface the upstream error.
- [id_token not returned on refresh] mitigated by keeping the prior `account_id`; only login populates it.

## Migration Plan

- Additive: `Tokens.account_id` is optional and defaults absent (older token files re-read fine; the next login populates it). No format migration.
- `auth = "oauth:codex"` previously built `OpenAiCompat` plus bearer (non-functional against Codex); it now builds `CodexResponses`. Static-key and other providers are unchanged.
- Rollback: unset `auth`/use a key, or point `FLEETY_CODEX_BACKEND_URL` elsewhere.

## Open Questions

- Exact `instructions` default and whether Codex requires specific `include`/`reasoning` fields for a plain turn — start minimal (empty `include`, no `reasoning`) per the proxy and adjust if the live backend needs more.
- Whether additional browser-simulation headers are needed in practice — include `User-Agent`; add others only if live testing shows they are required.
