## ADDED Requirements

### Requirement: HTTP, WebSocket, and SSE egress with SSRF guard

The system SHALL provide `fetch_url` (read-only GET), `http_request` (GET/POST/PUT/PATCH/DELETE/HEAD with headers, body xor multipart, retry, redirect/TLS controls, and `stream_to_file`), `ws_call` (one-shot WebSocket send/receive), and `sse_stream` (subscribe to a `text/event-stream`). All four SHALL only allow `http`/`https` (or `ws`/`wss` for `ws_call`) and SHALL refuse loopback, RFC1918, and IPv6 ULA/link-local hosts unless `FLEETY_ALLOW_PRIVATE_NET=1` is set.

#### Scenario: private host blocked by default

- **WHEN** `http_request` targets `http://127.0.0.1:8080` and `FLEETY_ALLOW_PRIVATE_NET` is not set
- **THEN** the call is refused with an SSRF-guard error

### Requirement: Persistent named cookie jars

`http_request`, `ws_call`, and `sse_stream` SHALL accept a `cookie_jar` name that persists cookies across calls in the managed store, so a session-bound or OAuth-authenticated API stays logged in.

#### Scenario: cookie persists across two calls

- **WHEN** two `http_request` calls pass the same `cookie_jar` name and the first receives a `Set-Cookie`
- **THEN** the second call sends that cookie
