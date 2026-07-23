## ADDED Requirements

### Requirement: Multi-owner reads preserve available results and owner failures

A read requesting multiple owners SHALL query each requested owner independently, return every available result, identify every unavailable or failed owner, and exit non-zero when any requested owner failed. It SHALL NOT synthesize defaults for a failed owner or suppress successful owner data.

#### Scenario: list remains useful with daemon offline

- **WHEN** `fleety config list` can read CLI and Server settings but the selected Daemon is offline
- **THEN** it SHALL output CLI and Server settings, report Daemon unavailable with remediation, and exit 1

### Requirement: Configuration results identify the actual owner context

Human and machine output for config reads and mutations SHALL identify the resolved owner, selected profile or device, and endpoint when applicable. This presentation requirement SHALL NOT expose credentials or change owner inference.

#### Scenario: automatic server routing is visible

- **WHEN** the user sets a Server-owned key without `--owner`
- **THEN** the result SHALL state that it was applied by the Server for the resolved profile and SHALL NOT imply that the CLI edited a local file

### Requirement: Provider credentials are Server-owned write-only secrets

Provider snapshots SHALL omit every plaintext API key and SHALL expose only non-secret key-presence metadata. Provider Apply SHALL distinguish Keep, Set, and Clear explicitly, merge the operation under the Server configuration transaction lease, and fail closed when either endpoint does not support the write-only protocol. A redacted or omitted key SHALL mean Keep and SHALL never silently clear an existing Server credential.

#### Scenario: redacted snapshot round-trip keeps the key

- **GIVEN** an API Provider has a Server-stored key
- **WHEN** an authenticated client reads and reapplies its unchanged Provider snapshot
- **THEN** no response byte contains the key and the stored key remains unchanged

#### Scenario: explicit clear removes the key

- **WHEN** the user confirms `provider set <name> --clear-key` or the equivalent Provider editor action
- **THEN** the apply payload SHALL carry a separate Clear intent and the Server SHALL remove only that Provider's key

### Requirement: Provider credential operations require an authenticated owner boundary

The Server SHALL reject model-catalog operations before reading or using any stored Provider API key or OAuth token when Server authentication is disabled or the caller is not authenticated. The failure SHALL be typed and actionable and SHALL cause zero outbound Provider requests.

#### Scenario: auth-disabled catalog cannot proxy a stored key

- **GIVEN** a Provider has a stored key and Server authentication is disabled
- **WHEN** a client requests that Provider's model catalog
- **THEN** the Server SHALL return an unauthenticated or auth-disabled error without contacting the Provider endpoint

### Requirement: Structured configuration mutations require an authenticated owner boundary

The Server SHALL apply one shared authentication gate to structured `ConfigApply` before dispatching to the Server or a selected Device owner. Local targets SHALL retain their `invalid` response. When Server authentication is disabled, a Server target SHALL reject Provider payloads and any non-`Keep` change, while a Device target SHALL reject every `ConfigApply`, including empty and all-`Keep` payloads because the Device owner persists every apply. Rejection SHALL return a typed actionable error and SHALL NOT write Server configuration, route `RunTool`, or otherwise contact a Daemon owner. When authentication is required, the existing authenticated connection and owner validation SHALL continue unchanged.

#### Scenario: auth-disabled Device apply sends no owner frame

- **GIVEN** Server authentication is disabled and device `remote-B` is connected
- **WHEN** a client sends structured `ConfigApply` for `remote-B`
- **THEN** the Server SHALL reject it before owner dispatch and the device SHALL receive zero `RunTool` frames

#### Scenario: auth-disabled owner matrix preserves non-writing behavior

- **GIVEN** Server authentication is disabled
- **WHEN** a client applies an empty or all-`Keep` payload to the Server target
- **THEN** the no-op SHALL continue without a write
- **AND WHEN** the same payload targets a Device
- **THEN** the Server SHALL reject it before owner dispatch
- **AND WHEN** the payload targets Local
- **THEN** the Server SHALL retain the `invalid` owner response

### Requirement: Provider and model mutations report activation timing

A successful Provider or model mutation SHALL report `NextConnection`. Read-only Provider and model queries SHALL report no effect.

#### Scenario: model role update names its effect

- **WHEN** `fleety model set` succeeds
- **THEN** human output SHALL explain that the change applies on the next connection and JSON SHALL encode the same effect
