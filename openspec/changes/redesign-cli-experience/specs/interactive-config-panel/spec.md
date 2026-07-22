## ADDED Requirements

### Requirement: Settings use owner-aware navigation and state

The Settings route SHALL provide Connection, CLI, Daemon, Server, and Providers & Models pages. Each page SHALL identify the selected profile and destination owner and SHALL represent Loading, Available, Dirty, Applying, Conflict, Failed, and Unavailable states explicitly. Storage filenames SHALL NOT be used as the primary page title or save destination.

#### Scenario: provider page names the connected Server

- **WHEN** the user opens Providers & Models while profile `office` is connected
- **THEN** the page SHALL identify `office` and its Server endpoint and SHALL NOT describe the action as editing `providers.toml`

### Requirement: Settings stage and apply changes per owner

CLI, Daemon, Server, and Provider/Model edits SHALL be staged before persistence. Apply SHALL act on exactly one owner, use that owner's persistence path, and report Saved, Restart required, Conflict, or Failed. Dirty state from separate owners SHALL remain separate and SHALL NOT be presented as one atomic transaction.

#### Scenario: failed remote apply retains the edit

- **WHEN** a Server apply fails or conflicts
- **THEN** its staged values SHALL remain Dirty or Conflict, the error SHALL remain visible, and no CLI or Daemon file SHALL be modified

##### Example: stale Server revision

- **GIVEN** Server revision `r1` is staged while the owner has advanced to `r2`
- **WHEN** Apply returns a typed conflict
- **THEN** the staged `r1` edits and remediation remain visible, and CLI and Daemon bytes remain unchanged

### Requirement: Profile switching resolves dirty remote state before reconnect

Switching profiles while Server or Daemon state is dirty SHALL require Apply, Discard, or Cancel and SHALL identify the old profile. Apply must succeed before switching; Discard SHALL clear only old-profile staged remote state. After selection, the old transport SHALL close and fresh Server and Daemon snapshots SHALL load from the selected profile.

#### Scenario: cancel keeps profile and edits

- **GIVEN** profile `A` has dirty Server settings
- **WHEN** the user selects profile `B` and chooses Cancel
- **THEN** profile `A` SHALL remain selected, its staged changes SHALL remain, and no reconnect SHALL occur

#### Scenario: failed new connection never reuses old snapshots

- **WHEN** the user discards old staged state, selects profile `B`, and `B` cannot connect
- **THEN** remote pages SHALL become Unavailable and SHALL NOT display or apply profile `A` snapshots
