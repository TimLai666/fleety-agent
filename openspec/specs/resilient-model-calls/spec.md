# resilient-model-calls Specification

## Purpose

TBD - created by archiving change 'resilient-model-calls'. Update Purpose after archive.

## Requirements

### Requirement: Transient model-call failures are retried with backoff

A model call SHALL retry on transient failures — HTTP 429, 408, 425, and 5xx (500, 502, 503, 504), plus connection and timeout errors — using exponential backoff with full jitter, up to a configurable maximum number of attempts. When the response carries a `Retry-After` header (in seconds), the call SHALL wait the indicated duration instead of the computed backoff. The backoff schedule SHALL be a pure function of (attempt, base, cap, retry-after, injected jitter) so it is unit-testable without a clock or live network.

#### Scenario: a 503 then success

- **WHEN** a model endpoint returns 503 on the first attempt and 200 on the second
- **THEN** the call waits a backoff delay and retries, and returns the successful response rather than an error

#### Scenario: Retry-After is honored

- **WHEN** a 429 response includes `Retry-After: 2`
- **THEN** the next attempt waits about 2 seconds (rather than the computed backoff)

##### Example: classification

| HTTP status / condition | Retryable? |
|---|---|
| 429, 408, 425, 500, 502, 503, 504 | yes |
| connection reset / timeout | yes |
| 400, 401, 403, 404 | no |
| 2xx | n/a (success) |


<!-- @trace
source: resilient-model-calls
updated: 2026-06-29
code:
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/retry.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
  - crates/agent-core/src/openai.rs
-->

---
### Requirement: Non-retryable failures fail fast

A model call SHALL NOT retry on non-retryable HTTP errors — 4xx other than 429/408/425 (e.g. 400, 401, 403, 404). It SHALL return the error immediately as a `CoreError::Provider` carrying the status and the existing remediation hint.

#### Scenario: 401 is not retried

- **WHEN** a model endpoint returns 401 Unauthorized
- **THEN** the call returns an error immediately without any retry


<!-- @trace
source: resilient-model-calls
updated: 2026-06-29
code:
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/retry.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
  - crates/agent-core/src/openai.rs
-->

---
### Requirement: Retries are bounded and configurable

The maximum retry count and the base/cap of the backoff SHALL be configurable via `FLEETY_*` settings registered in the typed config registry, with conservative defaults. Setting the retry count to 0 SHALL make a model call behave as a single request (the prior behavior). When retries are exhausted, the call SHALL return a `CoreError::Provider` whose message indicates the attempts were exhausted, and SHALL NOT panic.

#### Scenario: retries exhausted

- **WHEN** every attempt up to the configured maximum returns 503
- **THEN** the call returns a `CoreError::Provider` (never panics) indicating the retries were exhausted

#### Scenario: retries disabled

- **WHEN** the retry count is configured to 0 and the endpoint returns 503
- **THEN** the call makes exactly one request and returns the error (single-request behavior)


<!-- @trace
source: resilient-model-calls
updated: 2026-06-29
code:
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/retry.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
  - crates/agent-core/src/openai.rs
-->

---
### Requirement: Streaming retries only before output begins

For a streaming model call, a retry SHALL occur only when the failure happens before any delta has been emitted (connection setup or the initial HTTP status). Once streaming output has begun, the call SHALL NOT retry; it SHALL end with an error instead, so no already-emitted output is duplicated.

#### Scenario: failure after first delta is not retried

- **WHEN** a streaming call has already emitted one or more deltas and the stream then errors
- **THEN** the call ends with an error and does not restart the request

<!-- @trace
source: resilient-model-calls
updated: 2026-06-29
code:
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/retry.rs
  - crates/agent-core/src/lib.rs
  - crates/fleety-tools/src/config.rs
  - docs/env.md
  - crates/agent-core/src/openai.rs
-->