## ADDED Requirements

### Requirement: Providers report their modality capabilities

A model provider SHALL report which input modalities it supports (text always; image, audio, and PDF optionally) via a capability query. The capability set SHALL come from configuration when set (`FLEETY_MODEL_MODALITIES` / `FLEETY_CHEAP_MODEL_MODALITIES`, comma-separated), and otherwise be derived from a model-family heuristic. Parsing the configured modality string SHALL be a pure function so it is unit-testable.

#### Scenario: capabilities from configuration

- **WHEN** `FLEETY_MODEL_MODALITIES` is set to `text,image`
- **THEN** the main provider reports support for text and image, and not for audio or PDF

#### Scenario: capabilities from heuristic when unset

- **WHEN** no modality config is set and the model name matches a known multimodal family
- **THEN** the provider reports multimodal support (image/audio/pdf) as the default

##### Example: modality parsing

| Input string | image | audio | pdf |
|---|---|---|---|
| "text,image" | yes | no | no |
| "text,image,audio,pdf" | yes | yes | yes |
| "" (empty → heuristic) | (heuristic) | (heuristic) | (heuristic) |
| "text,bogus" | no | no | no |

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
