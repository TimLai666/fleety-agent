## 1. Reproduce and fix the backend identity mismatch

- [x] 1.1 Test the emitted HTTP request with a request-capture regression assertion that fails while the Codex model catalog sends the authorize-flow `fleety` originator and proves the request also carries client version, bearer, and account ID.
- [x] 1.2 Implement the requirement "OAuth model discovery uses the authenticated backend identity" and keep separate defaults for separate request classes by splitting the backend originator from the authorize-flow originator so catalog requests default to `codex_cli_rs`, authorize URLs remain `fleety`, and the existing environment override remains supported.

## 2. Synchronize documentation and verify

- [x] 2.1 Update `docs/env.md` to cover both Responses and model catalog requests, then run OAuth tests, workspace formatting, Clippy, Spectra validation, and the relevant CLI/server test suites.
