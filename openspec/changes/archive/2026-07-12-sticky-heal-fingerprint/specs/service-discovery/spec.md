## ADDED Requirements

### Requirement: The server advertises a persistent identity fingerprint

The server SHALL generate a persistent identity id on first start (stored under the agent home, stable across restarts and address changes; an unreadable or unwritable id file degrades to a per-run id with a warning and never crashes the server). The id SHALL be advertised as an mDNS TXT property alongside the existing version property, and SHALL be carried in `Welcome` as an additive optional field, sourced from the same value. Discovery (both the single-result fallback and the collecting scan) SHALL surface the advertised fingerprint on each found server; a server that advertises none yields a fingerprint-less entry.

#### Scenario: fingerprint is stable across restart and address change

- **WHEN** the server restarts on a different IP
- **THEN** it advertises the same identity fingerprint as before

#### Scenario: discovery carries the fingerprint

- **WHEN** a collecting scan resolves a server that advertises an identity fingerprint
- **THEN** the returned entry carries that fingerprint alongside the name and URL

#### Scenario: an old server yields a fingerprint-less entry

- **WHEN** a scan resolves a server that advertises no fingerprint
- **THEN** the entry's fingerprint is absent and no client treats it as any pinned identity
