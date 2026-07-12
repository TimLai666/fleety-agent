# self-update Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

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


<!-- @trace
source: update-autonomy
updated: 2026-07-12
code:
  - scripts/install-server.sh
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/update.rs
  - crates/fleety-daemon/src/poll_updates.rs
  - docs/env.md
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/mdns.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-protocol/src/lib.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Sidecar and install paths

The runtime SHALL read `FLEETY_INSYRA_BIN` for the path to the `fleety-insyra` Go sidecar (default: beside the executable) and `FLEETY_INSYRA_URL` to override its download URL for install/update. `FLEETY_INSTALL_DIR` SHALL set where the server install script lands the binary; when unset the script SHALL install to `/usr/local/bin` only when it can actually create a file there — verified by an atomic write probe (create then remove a temporary file), not a bare `-w` test that misreports an absent or root-owned directory — otherwise falling back to `$HOME/.local/bin`. Whichever directory is chosen, when it is not on `PATH` the script SHALL warn and print how to add it.

#### Scenario: sidecar resolves beside the executable

- **WHEN** `FLEETY_INSYRA_BIN` is unset
- **THEN** the `insyra_exec` tool spawns the `fleety-insyra` binary located beside the running executable

#### Scenario: install falls back when /usr/local/bin is not truly writable

- **WHEN** the install script runs without `FLEETY_INSTALL_DIR` and `/usr/local/bin` cannot actually be written (it is root-owned or absent)
- **THEN** the script installs into `$HOME/.local/bin`, and when that directory is not on `PATH` it warns and prints how to add it

<!-- @trace
source: cli-clipboard-acp-polish
updated: 2026-07-10
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/config.rs
  - docs/env.md
  - crates/fleety-server/src/restart_watch.rs
  - Dockerfile
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/privacy.rs
  - scripts/install.sh
  - crates/fleety-cli/src/main.rs
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/clipboard.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Update manifest schema

An update manifest SHALL be a JSON document in one of two forms: the flat form with string fields `version`, `url`, and `sha256` describing a single artifact, or the multi-target form with a string field `version`, a `targets` object mapping Rust target triples to entries holding `url` and `sha256`, and an optional string field `versioned_manifest` holding a URL template. Parsers SHALL ignore unknown fields. The updater SHALL select the `targets` entry whose key equals its own compile-time target triple, resolved from the shared target-triple table in the deps module (one table serves both the insyra sidecar and the updater). When the manifest carries no artifact for the local triple, installing SHALL fail with an error naming the manifest version and the local triple, while version probing on the same manifest SHALL still succeed. Downloaded artifact bytes SHALL be verified against the manifest `sha256` (case-insensitive hexadecimal comparison) before the executable swap, and the artifact SHALL be the raw executable for the local platform (the updater performs no archive extraction).

#### Scenario: multi-target manifest selects the local triple

- **WHEN** the updater on x86_64-unknown-linux-gnu parses a multi-target manifest whose `targets` holds entries for x86_64-unknown-linux-gnu and aarch64-apple-darwin
- **THEN** it resolves the `url` and `sha256` of the x86_64-unknown-linux-gnu entry

#### Scenario: manifest without the local triple

- **WHEN** the updater parses a multi-target manifest whose `targets` lacks the local triple
- **THEN** version probing returns the manifest version
- **WHEN** an install is attempted from that manifest
- **THEN** it fails with an error naming the manifest version and the local triple, and no executable swap happens

#### Scenario: flat manifest stays supported

- **WHEN** the updater parses a flat manifest with `version`, `url`, and `sha256` fields
- **THEN** it resolves that single artifact regardless of platform, with the `sha256` normalized to lowercase

<!-- @trace
source: update-manifest-pipeline
updated: 2026-07-11
code:
  - crates/fleety-tools/src/update.rs
  - crates/fleety-tools/src/deps.rs
  - crates/fleety-tools/src/deps/insyra.rs
  - crates/fleety-daemon/src/main.rs
  - .github/workflows/release.yml
  - docs/env.md
  - docs/STATUS.md
-->

---
### Requirement: Manifest URL templating

`FLEETY_UPDATE_MANIFEST` SHALL hold a single URL or URL template serving every resolution mode. The updater SHALL substitute `{bin}` with the name of the binary being updated. For latest resolution (background polling, `fleetyd update`, `fleety update`), the updater SHALL substitute `{version}` with the literal string `latest`. For pinned resolution, the updater SHALL substitute `{version}` with the exact target version and SHALL fail when the template lacks `{version}`. A template without `{bin}` SHALL be treated as the manifest of the running binary only: the updater SHALL NOT resolve a manifest for a different binary from a template lacking `{bin}`, and SHALL skip that binary's update with a warning naming the missing `{bin}` placeholder.

#### Scenario: latest resolution of a versioned template

- **WHEN** `FLEETY_UPDATE_MANIFEST` is `https://host/dl/{bin}/{version}/manifest.json` and fleetyd resolves its latest manifest URL
- **THEN** the resolved URL is `https://host/dl/fleetyd/latest/manifest.json`

##### Example: substitution matrix

| Template                             | Binary               | Mode            | Resolved URL                          |
| ------------------------------------ | -------------------- | --------------- | ------------------------------------- |
| https://h/dl/{bin}/latest.json       | fleety-server        | latest          | https://h/dl/fleety-server/latest.json |
| https://h/dl/{bin}/{version}/m.json  | fleetyd              | latest          | https://h/dl/fleetyd/latest/m.json     |
| https://h/dl/{bin}/{version}/m.json  | fleetyd              | pinned to 0.3.0 | https://h/dl/fleetyd/0.3.0/m.json      |
| https://h/fleetyd.json               | fleetyd (running)    | latest          | https://h/fleetyd.json                 |

#### Scenario: sibling update requires the bin placeholder

- **WHEN** the daemon updates sibling binaries and `FLEETY_UPDATE_MANIFEST` holds `{version}` but not `{bin}`
- **THEN** each sibling is skipped with a warning naming the missing `{bin}` placeholder, and the daemon's own self-update still proceeds

<!-- @trace
source: update-manifest-pipeline
updated: 2026-07-11
code:
  - crates/fleety-tools/src/update.rs
  - crates/fleety-tools/src/deps.rs
  - crates/fleety-tools/src/deps/insyra.rs
  - crates/fleety-daemon/src/main.rs
  - .github/workflows/release.yml
  - docs/env.md
  - docs/STATUS.md
-->

---
### Requirement: Fleet convergence version resolution

When a device daemon connects and the server reports a strictly newer version V, the daemon SHALL resolve V's manifest for each locally installed fleety binary in this order: first, when the env template contains `{version}`, via pinned template substitution; otherwise via the binary's latest manifest, used directly when its `version` equals V, else via that manifest's `versioned_manifest` template with `{bin}` and `{version}` substituted; when no path applies, the daemon SHALL log a warning naming both remedies (adding a `versioned_manifest` field to the published manifest, or switching the env template to a `{version}` form) and leave the binary unchanged. A manifest fetched to pin version V whose `version` field differs from V SHALL be rejected with a warning naming both versions, and nothing SHALL be installed from it. Convergence SHALL remain forward-only: a device SHALL NOT auto-downgrade, and a device newer than the server SHALL only warn.

#### Scenario: latest manifest already matches the server version

- **WHEN** the server reports version 0.3.0 and the latest manifest for the binary declares version 0.3.0
- **THEN** the daemon installs from that manifest without fetching a second manifest

#### Scenario: pinning through the versioned_manifest template

- **WHEN** the server reports 0.3.0, the latest manifest declares 0.4.0, and it carries `versioned_manifest` of `https://h/dl/{bin}/{version}/m.json`
- **THEN** the daemon fetches `https://h/dl/fleetyd/0.3.0/m.json` for fleetyd and installs from it after confirming its `version` field equals 0.3.0

#### Scenario: mismatched pinned manifest is rejected

- **WHEN** the manifest fetched to pin 0.3.0 declares version 0.4.0
- **THEN** the daemon rejects it with a warning naming both versions and installs nothing

#### Scenario: no pinning path

- **WHEN** the env template lacks `{version}` and the latest manifest neither matches the server version nor carries `versioned_manifest`
- **THEN** the daemon logs a warning naming both remedies and leaves the binary unchanged

<!-- @trace
source: update-manifest-pipeline
updated: 2026-07-11
code:
  - crates/fleety-tools/src/update.rs
  - crates/fleety-tools/src/deps.rs
  - crates/fleety-tools/src/deps/insyra.rs
  - crates/fleety-daemon/src/main.rs
  - .github/workflows/release.yml
  - docs/env.md
  - docs/STATUS.md
-->

---
### Requirement: Release publishes update artifacts and manifests

Each tagged release SHALL attach, for every supported Rust release target, raw per-binary executables for fleety, fleety-server, and fleetyd named as the binary name, a hyphen, and the target triple (with an `.exe` suffix on Windows), alongside the existing archives. Each tagged release SHALL also attach one manifest asset per binary, named as the binary name plus `-manifest.json`, in the multi-target schema: its artifact URLs SHALL point at the tag-pinned release assets (never a latest alias), its `sha256` values SHALL be computed from the exact bytes attached to the release, and its `versioned_manifest` field SHALL hold the release-download URL template resolvable for any tagged version. The release workflow SHALL fail before attaching any manifest when the tag version (leading `v` stripped) differs from the workspace package version read from the `[workspace.package]` section. A `workflow_dispatch` run SHALL generate and validate the manifests as workflow artifacts without attaching anything to a release.

#### Scenario: release asset set

- **WHEN** a release tag vX.Y.Z is pushed and the workflow succeeds
- **THEN** the release carries fleety-manifest.json, fleety-server-manifest.json, and fleetyd-manifest.json alongside raw binaries for every supported target, and each manifest declares version X.Y.Z

#### Scenario: tag and workspace version mismatch

- **WHEN** the pushed tag is v0.3.0 and the workspace package version is 0.2.0
- **THEN** the manifest job fails and no manifest asset is attached to the release

#### Scenario: dispatch dry-run

- **WHEN** the workflow runs via workflow_dispatch
- **THEN** manifests are generated and validated as workflow artifacts and no release asset is attached

<!-- @trace
source: update-manifest-pipeline
updated: 2026-07-11
code:
  - crates/fleety-tools/src/update.rs
  - crates/fleety-tools/src/deps.rs
  - crates/fleety-tools/src/deps/insyra.rs
  - crates/fleety-daemon/src/main.rs
  - .github/workflows/release.yml
  - docs/env.md
  - docs/STATUS.md
-->

---
### Requirement: The server updates itself in place

`fleety-server update` SHALL self-update the server binary from its update manifest (resolved through the `{bin}` template), refresh the fleety-insyra sidecar regardless of whether a new binary was installed, and — when a new binary was installed — trigger the existing idle-deferred restart so the new version takes over without interrupting an in-flight turn. Without a configured update manifest it SHALL fail with the existing actionable message. The server install script SHALL mention the command.

#### Scenario: a server-only host updates with one command

- **WHEN** `fleety-server update` runs on a host with a newer release published
- **THEN** the binary is downloaded, verified, swapped, the sidecar refreshed, and an idle-deferred restart is scheduled

#### Scenario: already up to date still refreshes the sidecar

- **WHEN** `fleety-server update` runs and the manifest matches the running version
- **THEN** it reports up-to-date and still refreshes the sidecar


<!-- @trace
source: update-autonomy
updated: 2026-07-12
code:
  - scripts/install-server.sh
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/update.rs
  - crates/fleety-daemon/src/poll_updates.rs
  - docs/env.md
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/mdns.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-protocol/src/lib.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Daemon updates carry the host's sibling binaries

`fleetyd update` and the polling loop's apply path SHALL, after updating the daemon itself and its sidecar, also update the host's sibling fleety binaries (`fleety` and `fleety-server`, whichever exist next to the executable) to the latest manifest version — gated on a `{bin}` manifest template exactly like the convergence path (a template without `{bin}` SHALL skip siblings with a note naming the fix, never resolving another binary's manifest). A sibling `fleety-server` that was updated SHALL be restarted through its idle-deferred restart. The host-wide sibling update SHALL be one shared implementation used by both the CLI's `fleety update` and the daemon paths.

#### Scenario: fleetyd update lifts the whole host

- **WHEN** `fleetyd update` runs on a host that also has `fleety` and `fleety-server` installed and the manifest template contains `{bin}`
- **THEN** all three binaries end up on the latest version and the server is restarted deferred

#### Scenario: a bin-less template skips siblings safely

- **WHEN** the manifest template lacks `{bin}`
- **THEN** the daemon updates only itself and prints the note about adding `{bin}`, and no sibling executable is touched

<!-- @trace
source: update-autonomy
updated: 2026-07-12
code:
  - scripts/install-server.sh
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/update.rs
  - crates/fleety-daemon/src/poll_updates.rs
  - docs/env.md
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/mdns.rs
  - README.md
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-protocol/src/lib.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->