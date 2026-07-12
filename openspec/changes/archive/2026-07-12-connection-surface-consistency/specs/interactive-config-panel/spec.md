## ADDED Requirements

### Requirement: The panel Connection region offers the local server

When the three-region `fleety config` panel opens, it SHALL probe for a local server on loopback with a short timeout and, when one answers and no saved profile already points at it, list a `local` entry in the Connection region (in addition to the saved profiles) using the same discovery the guided init uses. Selecting it with the existing switch/save keys SHALL make `local` the current profile and persist it — no pairing code is required because the local connection is loopback-trusted. When no local server answers, or a profile already points at it, the Connection region SHALL behave exactly as before (saved profiles only). The in-memory `local` entry SHALL NOT be written to disk unless the user saves.

#### Scenario: local server appears and is selectable

- **WHEN** the panel opens on a host whose local server answers and no profile points at it
- **THEN** a `local` entry appears in the Connection region, and switching to it and saving persists a `local` profile made current, without a pairing code

#### Scenario: no local server leaves the region unchanged

- **WHEN** the panel opens on a host with no local server, or a profile already targets the local URL
- **THEN** the Connection region lists only the saved profiles, as before

#### Scenario: an unsaved local entry is not persisted

- **WHEN** the panel shows the injected `local` entry but the user does not save
- **THEN** no `local` profile is written to connections.toml
