## Why

`codex-oauth` shipped ChatGPT login, token storage, and refresh, but the token can't yet drive a model: Codex authenticates against OpenAI's **Responses API** (`https://chatgpt.com/backend-api/codex/responses`), not `/chat/completions`. Fleety's only OpenAI-compatible provider (`OpenAiCompat`) speaks chat/completions, so `auth = "oauth:codex"` currently authenticates but cannot actually call the model. This change adds a Responses-API provider so a logged-in ChatGPT account can run the agent end-to-end.

## What Changes

- **New `CodexResponses` model provider** in `agent-core` implementing `ModelProvider` over the Responses API: builds the `input[]` + `instructions` + `tools` request shape, sets the Codex headers (`Authorization`, `chatgpt-account-id`, `OpenAI-Beta: responses=experimental`, `originator`, `session_id`), and parses the SSE stream (`response.output_text.delta`, `response.output_item.done`) into assistant text and tool calls.
- **Tool-calling over Responses**: map `ToolSpec` to Responses function tools, map assistant/tool history to `function_call` / `function_call_output` input items, and recover `function_call` items from the response — so the agent's tool loop works.
- **Capture the ChatGPT account id at login**: decode the OAuth `id_token` (JWT payload) for the `chatgpt_account_id` claim, persist it with the tokens, and expose bearer + account id to the provider.
- **Wire `auth = "oauth:codex"` to build `CodexResponses`** (pointed at the Codex backend base URL) instead of `OpenAiCompat`, so an OAuth-configured provider actually calls Codex.

## Non-Goals (optional)

- Non-Codex Responses API usage (a general Responses provider for arbitrary OpenAI accounts) — this targets the Codex ChatGPT backend specifically.
- API-key-authenticated Responses (`api.openai.com/v1/responses`) — out of scope; OAuth-backed Codex only.
- Reasoning-item surfacing / multimodal Responses parts beyond text and tool calls — deferred.
- Verifying against the live Codex backend from this build (network-gated, like SSH/CDP) — the request/SSE shapes are unit-tested offline against the documented contract.

## Capabilities

### New Capabilities

- `codex-responses-provider`: a Codex Responses-API model provider (OAuth bearer + account-id header, SSE streaming, tool-calling) that `auth = "oauth:codex"` builds, with the ChatGPT account id captured at login.

### Modified Capabilities

(none)

## Impact

- Affected specs: codex-responses-provider (new)
- Affected code:
  - New:
    - crates/agent-core/src/codex_responses.rs
  - Modified:
    - crates/agent-core/src/lib.rs
    - crates/agent-core/src/openai.rs
    - crates/fleety-tools/src/oauth.rs
    - crates/fleety-server/src/providers.rs
    - docs/env.md
  - Removed: (none)
- Dependencies: no new crates (reqwest SSE via `bytes_stream`, base64/serde_json already present; session id from the existing `uuid` dep). The Responses contract (endpoint, headers, request/SSE shapes) follows the documented Codex CLI behavior mirrored by codex-openai-proxy and heddle.
