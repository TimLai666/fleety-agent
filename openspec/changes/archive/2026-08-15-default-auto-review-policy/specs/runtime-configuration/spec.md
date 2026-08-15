## MODIFIED Requirements

### Requirement: Access policy and authentication

The server SHALL read `FLEETY_POLICY` (default `auto_review`). The accepted policies SHALL be `full_access`, `require_approval`, and `auto_review`. When `FLEETY_POLICY` is unset, empty, or unrecognized, the server SHALL use `auto_review`. Under `require_approval`, every non-read tool SHALL use the interactive approval flow. Under `auto_review`, read tools SHALL run directly and every mutate or critical tool SHALL use the unattended cheap-model review defined by the `auto-review` capability. Under `full_access`, mutate tools SHALL run directly with the existing audit and rollback behavior. The server SHALL read `FLEETY_REQUIRE_AUTH` (default `1`); any value other than an explicit `0` SHALL enable connection authentication, subject to the loopback trust behavior defined by `authentication-default-on`. `FLEETY_TOKEN` SHALL provide a bootstrap admin token usable to pair the first device. The server SHALL read `FLEETY_AUTO_REVIEW_TIMEOUT_SECS` with a positive default and SHALL use it as the maximum review wait; invalid or non-positive values SHALL use the documented default.

#### Scenario: auto review is the default

- **WHEN** `FLEETY_POLICY` is unset and no Server config value is stored
- **THEN** the policy is `auto_review`, read tools run directly, and mutate or critical tools enter the cheap-model review without a human approval request

##### Example: fresh server uses unattended review

- **GIVEN** a fresh Server has no `FLEETY_POLICY` environment variable and no persisted policy value
- **WHEN** it resolves its startup policy
- **THEN** it selects `auto_review` rather than `full_access`

#### Scenario: explicit full access remains available

- **WHEN** `FLEETY_POLICY=full_access`
- **THEN** the policy is `full_access` and mutate tools run without per-call approval

#### Scenario: explicit interactive approval remains available

- **WHEN** `FLEETY_POLICY=require_approval` and a mutate or critical tool is invoked
- **THEN** the call is routed through the interactive approval flow before executing

#### Scenario: explicit auto review remains selectable

- **WHEN** `FLEETY_POLICY=auto_review` and a mutate or critical tool is invoked
- **THEN** the call is routed through the cheap-model review and no human approval request is emitted

#### Scenario: invalid policy falls back safely

- **WHEN** `FLEETY_POLICY=RequireApproval` or another unrecognized value
- **THEN** the policy resolves to `auto_review` rather than `full_access`

#### Scenario: invalid auto-review timeout uses the safe default

- **WHEN** `FLEETY_AUTO_REVIEW_TIMEOUT_SECS` is unset, non-numeric, or non-positive
- **THEN** the server uses the documented positive default and never waits without a bound
