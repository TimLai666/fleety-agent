## 1. Runtime policy defaults

- [x] 1.1 Implement the **Policy resolver and registry both default to AutoReview** contract by changing `policy_from_env()` and the `FLEETY_POLICY` registry default so unset, empty, and unrecognized policies resolve to `auto_review`; verify with the resolver and configuration-registry unit tests.
- [x] 1.2 Implement the **Policy::default() matches runtime fallback** contract by making the core policy default `AutoReview`; verify with a focused `agent-core` policy-default test and existing gating tests.

## 2. User-facing policy contract

- [x] 2.1 Implement the **Explicit policy overrides remain stable** contract by preserving exact `full_access`, `require_approval`, and `auto_review` behavior and adding regression coverage for the explicit values and mixed-case fallback; verify with Server and config tests.
- [x] 2.2 Implement the **Documentation and specifications describe the same precedence** contract for `Access policy and authentication`, `Default policy uses auto review`, and `Auto review gates unattended tool execution` by updating the authoritative prompt, README, environment/tool references, and the three runtime/spec delta surfaces; verify with content review, `git diff --check`, and Spectra analysis.

## 3. Full validation

- [x] 3.1 Verify the **Implementation Contract** across all affected surfaces by running `cargo fmt --all`, targeted `cargo test -p agent-core --lib`, `cargo test -p fleety-tools --lib`, `cargo test -p fleety-server --bin fleety-server -- --test-threads=1`, and `spectra validate default-auto-review-policy`.
