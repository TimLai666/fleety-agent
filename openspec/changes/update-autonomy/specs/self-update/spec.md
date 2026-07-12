## ADDED Requirements

### Requirement: The server updates itself in place

`fleety-server update` SHALL self-update the server binary from its update manifest (resolved through the `{bin}` template), refresh the fleety-insyra sidecar regardless of whether a new binary was installed, and — when a new binary was installed — trigger the existing idle-deferred restart so the new version takes over without interrupting an in-flight turn. Without a configured update manifest it SHALL fail with the existing actionable message. The server install script SHALL mention the command.

#### Scenario: a server-only host updates with one command

- **WHEN** `fleety-server update` runs on a host with a newer release published
- **THEN** the binary is downloaded, verified, swapped, the sidecar refreshed, and an idle-deferred restart is scheduled

#### Scenario: already up to date still refreshes the sidecar

- **WHEN** `fleety-server update` runs and the manifest matches the running version
- **THEN** it reports up-to-date and still refreshes the sidecar

### Requirement: Daemon updates carry the host's sibling binaries

`fleetyd update` and the polling loop's apply path SHALL, after updating the daemon itself and its sidecar, also update the host's sibling fleety binaries (`fleety` and `fleety-server`, whichever exist next to the executable) to the latest manifest version — gated on a `{bin}` manifest template exactly like the convergence path (a template without `{bin}` SHALL skip siblings with a note naming the fix, never resolving another binary's manifest). A sibling `fleety-server` that was updated SHALL be restarted through its idle-deferred restart. The host-wide sibling update SHALL be one shared implementation used by both the CLI's `fleety update` and the daemon paths.

#### Scenario: fleetyd update lifts the whole host

- **WHEN** `fleetyd update` runs on a host that also has `fleety` and `fleety-server` installed and the manifest template contains `{bin}`
- **THEN** all three binaries end up on the latest version and the server is restarted deferred

#### Scenario: a bin-less template skips siblings safely

- **WHEN** the manifest template lacks `{bin}`
- **THEN** the daemon updates only itself and prints the note about adding `{bin}`, and no sibling executable is touched

## MODIFIED Requirements

### Requirement: Release-manifest update polling

The daemon SHALL poll a release manifest only when `FLEETY_UPDATE_MANIFEST` (a URL or URL template resolving to a JSON update manifest in either supported schema form) is set. `FLEETY_UPDATE_POLL_SECS` SHALL set the poll cadence (default `86400`, i.e. 24 hours) clamped to a 60-second floor. `FLEETY_AUTO_UPDATE` SHALL default to `apply` — each tick that finds a newer version runs the full host-wide update — and SHALL fall back to notify-only (log a warning, touch nothing) when set to `notify` or `0`.

#### Scenario: no manifest means no polling

- **WHEN** `FLEETY_UPDATE_MANIFEST` is unset
- **THEN** the daemon does not spawn the update poll loop

#### Scenario: apply by default, notify on request

- **WHEN** a newer version is found and `FLEETY_AUTO_UPDATE` is unset
- **THEN** the daemon runs the full update
- **WHEN** the same is found and `FLEETY_AUTO_UPDATE=notify`
- **THEN** the daemon logs a warning and does not self-update
