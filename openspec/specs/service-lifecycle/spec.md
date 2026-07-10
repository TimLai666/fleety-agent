# service-lifecycle Specification

## Purpose

TBD - created by archiving change 'service-lifecycle'. Update Purpose after archive.

## Requirements

### Requirement: Background service is controlled by the CLI

Both fleetyd and fleety-server SHALL be controllable as a background OS service through CLI verbs — install, uninstall, start, stop, restart, enable, disable, and status — that map to the platform service manager (systemd user on Linux, launchd on macOS, the Service Control Manager on Windows). When run as a service the process SHALL have no window or terminal, SHALL keep running after the launching terminal closes, and SHALL be single-instance (the manager will not start a second copy).

#### Scenario: start, then survive terminal close

- **WHEN** the user installs and starts the service, then closes the terminal
- **THEN** the process keeps running in the background with no window

##### Example: verb → manager intent

| CLI verb | Intent |
| -------- | ------ |
| start / stop / restart | run now / stop now / stop+start |
| enable / disable | turn boot (login) autostart on / off |
| install / uninstall | register / remove the service |
| status | report whether it is running and whether autostart is on |


<!-- @trace
source: service-lifecycle
updated: 2026-06-28
code:
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-server/src/winsvc.rs
-->

---
### Requirement: The daemon reconnects after disconnect or sleep

fleetyd SHALL keep itself connected to the server across transient disconnects and device sleep: when the connection drops (or fails to establish), it SHALL retry with exponential backoff (base delay, capped, with jitter) rather than exiting, and SHALL reset the backoff to the base after a successful connection. It SHALL NOT prevent the device from sleeping, and on a clean shutdown signal (Ctrl+C or the service Stop) it SHALL exit promptly.

#### Scenario: reconnects with growing backoff, resets on success

- **WHEN** the connection drops repeatedly and then succeeds
- **THEN** fleetyd waits a growing (capped, jittered) delay between attempts, keeps retrying instead of exiting, and resets the delay to the base after it reconnects

##### Example: backoff base delay by attempt (base 1s, factor 2, cap 30s, before jitter)

| Attempt (consecutive failures) | Base delay (seconds) |
| ------------------------------ | -------------------- |
| 1 | 1 |
| 2 | 2 |
| 3 | 4 |
| 4 | 8 |
| 5 | 16 |
| 6 | 30 |
| 7 | 30 |


<!-- @trace
source: service-lifecycle
updated: 2026-06-28
code:
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-server/src/winsvc.rs
-->

---
### Requirement: Boot autostart can be toggled

`enable` SHALL make the service start automatically at boot or login, and `disable` SHALL turn that off, without uninstalling the service or stopping the current run.

#### Scenario: disable autostart but keep running

- **WHEN** a running, autostart-enabled service is `disable`d
- **THEN** it keeps running now but no longer starts automatically at the next boot


<!-- @trace
source: service-lifecycle
updated: 2026-06-28
code:
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-server/src/winsvc.rs
-->

---
### Requirement: Windows runs as a real service

On Windows the background process SHALL run as a real Service Control Manager service (not a scheduled task), so start/stop/restart/enable have proper service semantics and the process runs with no window and survives logout. Installing the service SHALL require administrator rights; when they are missing the CLI SHALL fail with an actionable message telling the user to run it once as administrator.

#### Scenario: install without admin is actionable

- **WHEN** `install` runs on Windows without administrator rights
- **THEN** it does not crash and reports that the service install needs to be run once as administrator

#### Scenario: runs with no user logged in

- **WHEN** the SCM service is installed with auto-start and the machine boots with no user logged in
- **THEN** the service runs and does its headless work (serving, connecting, file/exec/MCP) without a login session

#### Scenario: desktop tools need an interactive session

- **WHEN** a desktop-bound tool (GUI control, screenshot, a visible browser) is invoked while the service runs headless with no interactive session
- **THEN** the tool fails gracefully with an actionable message (rather than hanging or crashing), since the Windows session has no desktop


<!-- @trace
source: service-lifecycle
updated: 2026-06-28
code:
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-server/src/winsvc.rs
-->

---
### Requirement: Server autostarts by default and offers up/down

fleety-server `install` SHALL enable boot autostart by default (the user can later `disable` it). fleety-server SHALL also provide `up` (install + enable + start, a one-command "running in the background" like `docker compose up -d`) and `down` (stop).

#### Scenario: one-command up

- **WHEN** the user runs `fleety-server up`
- **THEN** the server is installed, set to autostart, and started in the background, and the prompt returns while it keeps running


<!-- @trace
source: service-lifecycle
updated: 2026-06-28
code:
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-server/src/winsvc.rs
-->

---
### Requirement: Restart waits for in-flight work

A restart (manual or triggered by self-update) SHALL NOT interrupt in-flight work. The runtime that owns the workload SHALL record a pending restart and carry it out only when it is idle — for fleety-server, idle MUST mean its in-flight turn count (interactive turns and schedule-fired turns alike) is zero; for fleetyd, no running on-device tool — bounded by a deferral deadline after which it restarts anyway and a cooldown between restarts. Only an explicit `--force` restart SHALL bypass the deferral and restart immediately. An external `fleety-server restart` invocation against a running server SHALL request that running server to restart once idle rather than immediately stopping it through the service manager; when no server is running it SHALL fall back to a direct service-manager restart.

#### Scenario: a busy server defers the restart

- **WHEN** a non-forced `fleety-server restart` is invoked while a turn is in flight
- **THEN** the running server keeps serving the in-flight turn and carries out the restart only once its in-flight turn count reaches zero (or the deferral deadline is reached)

#### Scenario: forced restart is immediate

- **WHEN** the user runs `fleety-server restart --force`
- **THEN** the service restarts immediately through the service manager without waiting for idle

#### Scenario: external invocation requests rather than hard-kills

- **WHEN** a running fleety-server receives a non-forced `restart` invocation
- **THEN** the invoking CLI returns after recording a restart request and the running server SHALL NOT be stopped through the service manager until it becomes idle or reaches the deferral deadline

#### Scenario: no running server falls back to a manager restart

- **WHEN** `fleety-server restart` is invoked while no server process is running
- **THEN** it performs a direct service-manager restart, since there is no in-flight work to wait for


<!-- @trace
source: restart-defer-until-idle
updated: 2026-07-10
code:
  - crates/fleety-server/src/scheduler.rs
  - Dockerfile
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/identity.rs
  - docs/env.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/clipboard.rs
  - scripts/install.sh
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/service.rs
  - README.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Self-update restarts the service

When the daemon self-updates, it SHALL restart the service so the new binary takes effect — via the deferred-until-idle restart above so an update never interrupts in-flight work — swapping the binary in a way that works while it is running (on Windows, renaming the running executable aside and placing the new one before the restart).

#### Scenario: update applies after an idle restart

- **WHEN** a self-update completes for an installed service
- **THEN** the service restarts (once idle) and runs the new binary

<!-- @trace
source: service-lifecycle
updated: 2026-06-28
code:
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/Cargo.toml
  - docs/env.md
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-server/src/winsvc.rs
-->

---
### Requirement: Update-triggered server restart defers until idle

A restart of fleety-server triggered by an update — `fleety update` after swapping the server binary, or fleetyd converging a host to the server's version — SHALL route through the same deferred-until-idle path as a non-forced manual restart (never an immediate forced restart), so an update never interrupts an in-flight turn before the deferral deadline.

#### Scenario: update restarts the server once idle

- **WHEN** an update swaps the fleety-server binary on a running server that is mid-turn
- **THEN** the server restart is deferred and carried out once the server is idle (or the deferral deadline is reached), and the messaging tells the user the restart happens when idle rather than that a turn will be interrupted

<!-- @trace
source: restart-defer-until-idle
updated: 2026-07-10
code:
  - crates/fleety-server/src/scheduler.rs
  - Dockerfile
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/identity.rs
  - docs/env.md
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/clipboard.rs
  - scripts/install.sh
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-tools/src/service.rs
  - README.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Windows lifecycle verbs pre-flight an elevation check

On Windows, before invoking the Service Control Manager for any state-changing lifecycle verb (install, uninstall, start, stop, restart, enable, disable, up, down), the CLI SHALL detect whether the current process is running elevated (administrator). When it is not elevated, the CLI SHALL abort before issuing any `sc` command — leaving no partial service state — and SHALL print an actionable message on stderr telling the user to re-run the command from an elevated (Administrator) terminal. The elevation detection SHALL NOT use `unsafe` code and SHALL NOT add a new crate dependency. Query-only verbs (status) SHALL NOT require elevation and SHALL NOT perform the check.

#### Scenario: install without admin aborts before touching the SCM

- **WHEN** `install` or `up` runs on Windows without administrator rights
- **THEN** the CLI detects the missing elevation, exits non-zero before running any `sc` command, and prints a message on stderr telling the user to re-run it from an elevated terminal

#### Scenario: elevated run proceeds

- **WHEN** a state-changing lifecycle verb runs on Windows in an elevated terminal
- **THEN** the elevation check passes and the verb proceeds to invoke the Service Control Manager

#### Scenario: status needs no elevation

- **WHEN** `status` runs on Windows without administrator rights
- **THEN** the CLI performs no elevation check and reports the service status normally

<!-- @trace
source: deploy-hardening
updated: 2026-07-10
code:
  - crates/fleety-cli/src/main.rs
  - docs/env.md
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-tools/src/config.rs
  - Dockerfile
  - scripts/install.sh
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-cli/src/input.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->