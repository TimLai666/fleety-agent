## MODIFIED Requirements

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
