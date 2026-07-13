## MODIFIED Requirements

### Requirement: Login status and logout do not leak tokens

The CLI SHALL provide a `status [<provider>]` command and a `logout <provider>` command. `status <provider>` SHALL report whether the connected server holds that provider's credential and when it expires without printing the token values; `status` with no provider SHALL enumerate the `oauth:codex` providers in the connected server's provider config and report each one's signed-in state and expiry on its own line. The remote interactive provider editor SHALL reuse the same server-side credential status authority and SHALL show each `oauth:codex` provider as `signed in`, `not signed in`, or `unavailable` without printing token values. `logout <provider>` SHALL remove that provider's credential stored on the connected server. When a legacy local token file exists on the CLI host, status SHALL note that it is no longer used by any flow. The authorization/token endpoints, client id, and backend base URL SHALL be fixed constants in the code (OpenAI's public values, like the loopback port) ? they SHALL NOT be configuration keys or environment variables and SHALL NOT appear in `fleety config`.

#### Scenario: status reports one provider's state without token values

- **WHEN** a user runs status for a provider while the connected server holds that provider's credential
- **THEN** it reports the signed-in state and expiry of that provider's server-side credential without printing the access or refresh token

#### Scenario: status with no provider lists every oauth provider

- **WHEN** a user runs status with no provider argument
- **THEN** the CLI lists each `oauth:codex` provider with its signed-in state and expiry, one per line

#### Scenario: the remote editor reports provider auth state

- **WHEN** the remote interactive provider editor receives credential status for an `oauth:codex` provider
- **THEN** its provider row reports `signed in` or `not signed in` according to the server response, and reports `unavailable` when the status query cannot be completed

#### Scenario: logout removes one provider's credential

- **WHEN** a user runs logout for a provider
- **THEN** that provider's token store on the connected server is removed and subsequent OAuth calls for that provider report a logged-out state, while other providers' credentials are untouched

#### Scenario: leftover local file is flagged

- **WHEN** status runs on a CLI host that still has a legacy local token file
- **THEN** the output notes the file is no longer read by any flow and suggests re-running login

#### Scenario: endpoints are fixed, not configurable

- **WHEN** a user inspects or edits configuration (env, config.toml, or fleety config)
- **THEN** the Codex client id and endpoints are not present as settings; the login flow and provider always use the hardcoded constants

