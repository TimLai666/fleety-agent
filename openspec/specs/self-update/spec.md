# self-update Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: Release-manifest update polling

The daemon SHALL poll a release manifest only when `FLEETY_UPDATE_MANIFEST` (a JSON URL with `version`, `url`, `sha256`) is set. `FLEETY_UPDATE_POLL_SECS` SHALL set the poll cadence (default `86400`, i.e. 24 hours) clamped to a 60-second floor. `FLEETY_AUTO_UPDATE` SHALL default to `notify` (log a warning only); when set to `apply` the daemon SHALL run the full update on each tick.

#### Scenario: no manifest means no polling

- **WHEN** `FLEETY_UPDATE_MANIFEST` is unset
- **THEN** the daemon does not spawn the update poll loop

#### Scenario: notify versus apply

- **WHEN** a newer version is found and `FLEETY_AUTO_UPDATE` is unset
- **THEN** the daemon logs a warning and does not self-update
- **WHEN** the same is found and `FLEETY_AUTO_UPDATE=apply`
- **THEN** the daemon runs the full update


<!-- @trace
source: baseline-config-specs
updated: 2026-06-28
code:
  - .agents/skills/spectra-commit/SKILL.md
  - .opencode/skills/spectra-debug/SKILL.md
  - .opencode/commands/spectra-ingest.md
  - .opencode/skills/spectra-audit/SKILL.md
  - .agents/skills/spectra-discuss/SKILL.md
  - .agents/skills/spectra-archive/SKILL.md
  - .opencode/skills/spectra-ask/SKILL.md
  - .opencode/commands/spectra-drift.md
  - .opencode/commands/spectra-propose.md
  - .opencode/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-commit/SKILL.md
  - .opencode/commands/spectra-commit.md
  - .agents/skills/spectra-ask/SKILL.md
  - .agents/skills/spectra-audit/SKILL.md
  - .opencode/commands/spectra-debug.md
  - .agents/skills/spectra-drift/SKILL.md
  - .opencode/skills/spectra-archive/SKILL.md
  - .agents/skills/spectra-ingest/SKILL.md
  - .opencode/commands/spectra-audit.md
  - .opencode/commands/spectra-apply.md
  - .opencode/commands/spectra-discuss.md
  - .spectra.yaml
  - CLAUDE.md
  - .opencode/commands/spectra-ask.md
  - .opencode/skills/spectra-ingest/SKILL.md
  - .opencode/skills/spectra-discuss/SKILL.md
  - .opencode/skills/spectra-drift/SKILL.md
  - .opencode/commands/spectra-archive.md
  - .agents/skills/spectra-debug/SKILL.md
  - .agents/skills/spectra-propose/SKILL.md
  - .agents/skills/spectra-apply/SKILL.md
  - .opencode/skills/spectra-propose/SKILL.md
  - AGENTS.md
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