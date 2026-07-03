# codex-responses-provider Specification

## Purpose

TBD - created by archiving change 'codex-responses-provider'. Update Purpose after archive.

## Requirements

### Requirement: A Codex Responses provider calls the model over the Responses API

The system SHALL provide a model provider that talks to the Codex ChatGPT backend using the Responses API rather than the chat-completions API. It SHALL POST to the configured Codex backend URL with the request body shape the Responses API expects — a `model`, an `instructions` string, an `input` array of items, a `tools` array, and `stream` enabled — and SHALL set the Codex request headers (an OAuth bearer, the ChatGPT account id header, the Responses beta header, an originator, and a per-request session id). The provider SHALL implement the same provider interface as the existing providers so the agent loop uses it unchanged.

#### Scenario: a turn runs against the Codex backend

- **WHEN** the agent completes a turn with a Codex Responses provider that has valid credentials
- **THEN** the provider POSTs a Responses-shaped request to the Codex backend with the OAuth bearer and account-id headers and returns the assistant message

#### Scenario: system messages become instructions

- **WHEN** the conversation contains system messages
- **THEN** they are folded into the request's `instructions` field, not sent as `input` message items


<!-- @trace
source: codex-responses-provider
updated: 2026-07-03
code:
  - crates/fleety-tools/src/oauth.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - prompts/memory.md
  - crates/fleety-server/src/providers.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/Cargo.toml
  - README.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/main.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/gc.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: Tool calls work over the Responses API

The provider SHALL support tool-calling: it SHALL map each tool spec to a Responses function tool, map prior assistant tool calls to `function_call` input items and prior tool results to `function_call_output` input items, and SHALL recover function calls the model emits so the agent's tool loop proceeds.

#### Scenario: the model requests a tool

- **WHEN** the streamed response contains a completed `function_call` output item
- **THEN** the provider returns an assistant message carrying that tool call (name and parsed arguments) so the agent can run the tool

#### Scenario: a tool result is sent back

- **WHEN** the conversation history contains a tool result message
- **THEN** it is encoded as a `function_call_output` input item keyed by its call id


<!-- @trace
source: codex-responses-provider
updated: 2026-07-03
code:
  - crates/fleety-tools/src/oauth.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - prompts/memory.md
  - crates/fleety-server/src/providers.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/Cargo.toml
  - README.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/main.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/gc.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: The provider parses the Responses SSE stream

The provider SHALL read the server-sent-events stream and assemble the assistant output: it SHALL append text from output-text delta events (emitting each delta for live display), collect completed function-call items, and finish on the stream's completion or done marker. A malformed event line SHALL be skipped rather than fail the turn, and the provider SHALL NOT crash on stream errors.

#### Scenario: text streams in deltas

- **WHEN** the stream delivers output-text delta events
- **THEN** the provider appends them in order into the final assistant text and emits each delta to the live display callback

#### Scenario: a malformed line is skipped

- **WHEN** a data line in the stream is not valid JSON
- **THEN** the provider skips it and continues assembling the response


<!-- @trace
source: codex-responses-provider
updated: 2026-07-03
code:
  - crates/fleety-tools/src/oauth.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - prompts/memory.md
  - crates/fleety-server/src/providers.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/Cargo.toml
  - README.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/main.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/gc.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: The ChatGPT account id is captured at login

Login SHALL decode the OAuth `id_token` and persist the ChatGPT account id alongside the tokens, so the provider can send it as the account-id header. The account id SHALL be read from the `chatgpt_account_id` claim, falling back to the nested OpenAI auth claim and then to the first organization id. A refresh that returns no `id_token` SHALL keep the previously captured account id. Decoding SHALL be a pure function and SHALL yield nothing (not an error) on a malformed token.

#### Scenario: account id is decoded from the id_token

- **WHEN** login exchanges the authorization code and the token response includes an `id_token` carrying a `chatgpt_account_id`
- **THEN** that account id is stored with the tokens

#### Scenario: a malformed id_token yields no account id

- **WHEN** the `id_token` cannot be decoded into claims
- **THEN** the account id is absent (no crash), and the provider omits the account-id header


<!-- @trace
source: codex-responses-provider
updated: 2026-07-03
code:
  - crates/fleety-tools/src/oauth.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - prompts/memory.md
  - crates/fleety-server/src/providers.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/Cargo.toml
  - README.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/main.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/gc.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
-->

---
### Requirement: The oauth:codex auth mode builds the Responses provider

When a provider's authentication mode selects Codex OAuth, the runtime SHALL build the Codex Responses provider (pointed at the configured Codex backend URL, sourcing the bearer and account id from the OAuth token store) instead of the chat-completions provider. Other auth modes SHALL be unchanged. When no tokens are stored, a model call SHALL return an actionable "log in" error rather than crash.

#### Scenario: oauth:codex selects the Responses provider

- **WHEN** a provider is configured with the Codex OAuth auth mode
- **THEN** the runtime builds the Codex Responses provider for it, targeting the Codex backend

#### Scenario: default auth mode is unchanged

- **WHEN** a provider has no auth mode or a static-key mode
- **THEN** the runtime builds the existing chat-completions provider exactly as before

<!-- @trace
source: codex-responses-provider
updated: 2026-07-03
code:
  - crates/fleety-tools/src/oauth.rs
  - crates/agent-core/src/openai.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-protocol/src/lib.rs
  - docs/env.md
  - prompts/memory.md
  - crates/fleety-server/src/providers.rs
  - crates/fleety-server/src/sites.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-server/src/presence.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-cli/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-daemon/src/colocation.rs
  - crates/fleety-server/src/storage.rs
  - crates/agent-core/Cargo.toml
  - README.md
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/main.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/gc.rs
  - docs/tools.md
  - crates/fleety-server/src/scheduler.rs
-->