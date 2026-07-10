## MODIFIED Requirements

### Requirement: A pooled provider reports homogeneous capabilities

A pooled provider (a model role's member pool) SHALL report its modality capabilities as the **union** across its members: a modality is advertised as supported when **any** member supports it. It SHALL NOT report only the first member's capabilities, nor the intersection. Because each member degrades unsupported attachments inside its own call, the union lets a capable member receive an attachment natively while a less-capable member degrades it — so a mixed pool never blocks an attachment that some member could handle.

#### Scenario: a mixed pool advertises the union of member modalities

- **GIVEN** a pool whose first member is text-only and whose second member accepts images
- **WHEN** the pool's capabilities are queried (e.g. for the client's audio/image hint)
- **THEN** the image-capable modality is reported as supported, not suppressed by the first member being text-only
