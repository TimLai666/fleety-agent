## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: Update-triggered server restart defers until idle

A restart of fleety-server triggered by an update — `fleety update` after swapping the server binary, or fleetyd converging a host to the server's version — SHALL route through the same deferred-until-idle path as a non-forced manual restart (never an immediate forced restart), so an update never interrupts an in-flight turn before the deferral deadline.

#### Scenario: update restarts the server once idle

- **WHEN** an update swaps the fleety-server binary on a running server that is mid-turn
- **THEN** the server restart is deferred and carried out once the server is idle (or the deferral deadline is reached), and the messaging tells the user the restart happens when idle rather than that a turn will be interrupted
