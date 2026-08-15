## MODIFIED Requirements

### Requirement: Access policy and authentication

The server SHALL read `FLEETY_POLICY` (default `full_access`). The accepted policies SHALL be `full_access`, `require_approval`, and `auto_review`. Under `require_approval`, every non-read tool SHALL use the interactive approval flow. Under `auto_review`, read tools SHALL run directly and every mutate or critical tool SHALL use the unattended cheap-model review defined by the `auto-review` capability. The server SHALL read `FLEETY_REQUIRE_AUTH` (default `1`); any value other than an explicit `0` SHALL enable connection authentication, subject to the loopback trust behavior defined by `authentication-default-on`. `FLEETY_TOKEN` SHALL provide a bootstrap admin token usable to pair the first device. The server SHALL read `FLEETY_AUTO_REVIEW_TIMEOUT_SECS` with a positive default and SHALL use it as the maximum review wait; invalid or non-positive values SHALL use the documented default.

#### Scenario: full access remains the default

- **WHEN** `FLEETY_POLICY` is unset
- **THEN** the policy is `full_access` and mutate tools run without per-call approval

#### Scenario: interactive approval remains available

- **WHEN** `FLEETY_POLICY=require_approval` and a mutate or critical tool is invoked
- **THEN** the call is routed through the interactive approval flow before executing

#### Scenario: auto review is selectable

- **WHEN** `FLEETY_POLICY=auto_review` and a mutate or critical tool is invoked
- **THEN** the call is routed through the cheap-model review and no human approval request is emitted

#### Scenario: invalid auto-review timeout uses the safe default

- **WHEN** `FLEETY_AUTO_REVIEW_TIMEOUT_SECS` is unset, non-numeric, or non-positive
- **THEN** the server uses the documented positive default and never waits without a bound
