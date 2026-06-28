## ADDED Requirements

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

### Requirement: Boot autostart can be toggled

`enable` SHALL make the service start automatically at boot or login, and `disable` SHALL turn that off, without uninstalling the service or stopping the current run.

#### Scenario: disable autostart but keep running

- **WHEN** a running, autostart-enabled service is `disable`d
- **THEN** it keeps running now but no longer starts automatically at the next boot

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

### Requirement: Server autostarts by default and offers up/down

fleety-server `install` SHALL enable boot autostart by default (the user can later `disable` it). fleety-server SHALL also provide `up` (install + enable + start, a one-command "running in the background" like `docker compose up -d`) and `down` (stop).

#### Scenario: one-command up

- **WHEN** the user runs `fleety-server up`
- **THEN** the server is installed, set to autostart, and started in the background, and the prompt returns while it keeps running

### Requirement: Restart waits for in-flight work

A restart (including one triggered by self-update) SHALL NOT interrupt in-flight work. The runtime SHALL record a pending restart and carry it out only when the service is idle (no in-flight turn or running on-device tool), bounded by a deferral deadline after which it restarts anyway and a cooldown between restarts. A `force` restart (an explicit manual `restart`) SHALL bypass the deferral and restart immediately.

#### Scenario: a busy service defers the restart

- **WHEN** a non-forced restart is requested while work is in flight
- **THEN** the restart is deferred and carried out once the service becomes idle (or the deferral deadline is reached)

#### Scenario: forced restart is immediate

- **WHEN** the user runs `restart` (forced)
- **THEN** the service restarts immediately without waiting for idle

### Requirement: Self-update restarts the service

When the daemon self-updates, it SHALL restart the service so the new binary takes effect — via the deferred-until-idle restart above so an update never interrupts in-flight work — swapping the binary in a way that works while it is running (on Windows, renaming the running executable aside and placing the new one before the restart).

#### Scenario: update applies after an idle restart

- **WHEN** a self-update completes for an installed service
- **THEN** the service restarts (once idle) and runs the new binary
