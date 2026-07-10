# capability-aware-modality Specification

## Purpose

TBD - created by archiving change 'capability-aware-modality'. Update Purpose after archive.

## Requirements

### Requirement: Providers report their modality capabilities

Every provider SHALL report the input modalities it accepts (text, image, audio, video, pdf), derived from an explicit `modalities` setting or the model-family heuristic. A member pool SHALL report the **union** of its members' capabilities rather than any single member's, so the client's per-turn hints (e.g. whether to send audio) reflect what any routed member could accept. The native-vs-degrade decision for a given attachment SHALL happen inside the member that actually serves the call, using that member's own capabilities.

#### Scenario: a single provider reports its own capabilities

- **WHEN** a text-only provider is queried
- **THEN** it reports image/audio/video as unsupported

##### Example: a text-only member's capabilities

- **GIVEN** a provider member with `modalities = "text"`
- **WHEN** its capabilities are queried
- **THEN** `image`, `audio`, and `video` are all reported unsupported, `text` supported

#### Scenario: a pool reports the member union

- **WHEN** a pool mixing a text-only and an image-capable member is queried
- **THEN** it reports image as supported (the union), and the routed member degrades the attachment if it cannot serve it

##### Example: union of text-only + image-capable members

- **GIVEN** member A with `modalities = "text"` and member B with `modalities = "text,image"`
- **WHEN** the pool of `[A, B]` is queried
- **THEN** it reports `image` supported (union), not unsupported (which taking A's `first()` would give)


<!-- @trace
source: provider-model-two-tier
updated: 2026-07-10
code:
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/server.rs
  - crates/fleety-server/src/pool.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Unsupported attachments degrade gracefully instead of failing the turn

Before sending a message, the system SHALL route each attachment according to the provider's capabilities: a supported modality is sent as the provider's native media part; an unsupported modality SHALL NOT be sent as a media part and SHALL instead be replaced by a short text note describing the dropped attachment, so the turn proceeds rather than the endpoint rejecting it. An unknown MIME type SHALL continue to degrade to a text note. The mapping from MIME to modality and the support check SHALL be pure functions.

#### Scenario: image to a text-only model is dropped with a note

- **WHEN** a user message carries an image attachment and the resolved provider supports only text
- **THEN** no image media part is sent; a text note (e.g. that an image was attached but is unreadable) is included, and the call is not rejected for the attachment

#### Scenario: image to a multimodal model is sent normally

- **WHEN** a user message carries an image attachment and the provider supports image
- **THEN** the image is sent as the provider's native image part, as before

#### Scenario: existing behavior preserved by default

- **WHEN** a provider has not been given explicit capabilities (e.g. a test double defaulting to full support)
- **THEN** attachments route exactly as they did before this change

<!-- @trace
source: capability-aware-modality
updated: 2026-06-29
code:
  - crates/agent-core/src/lib.rs
  - docs/env.md
  - crates/agent-core/src/gemini.rs
  - crates/agent-core/src/model.rs
  - crates/fleety-tools/src/config.rs
  - crates/agent-core/src/retry.rs
  - crates/fleety-server/src/providers.rs
  - crates/agent-core/src/openai.rs
-->