## ADDED Requirements

### Requirement: OpenAI-compatible model endpoint

The runtime SHALL read `FLEETY_MODEL_BASE_URL` (the OpenAI-compatible `/v1` root), `FLEETY_MODEL` (the model name), and `FLEETY_MODEL_KEY` (the bearer token when the endpoint needs one). When `FLEETY_MODEL_BASE_URL` and `FLEETY_MODEL` are unset, the runtime SHALL fall back to a local echo provider rather than failing to start. `FLEETY_MODEL_STREAM` SHALL default to `0`; when set to `1` the runtime SHALL use the SSE streaming endpoint for token-by-token output.

#### Scenario: unset provider falls back to echo

- **WHEN** the server starts with `FLEETY_MODEL_BASE_URL` and `FLEETY_MODEL` unset
- **THEN** it runs with the echo provider instead of refusing to start

#### Scenario: streaming opt-in

- **WHEN** `FLEETY_MODEL_STREAM=1`
- **THEN** the runtime requests the SSE streaming endpoint
