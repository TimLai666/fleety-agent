# startup-dependencies Specification

## Purpose

TBD - created by archiving change 'startup-dependencies'. Update Purpose after archive.

## Requirements

### Requirement: Startup checks and ensures dependencies

On startup, fleetyd and fleety-server SHALL each check the external dependencies their features need and, when one is missing, ensure it via the dependency's strategy. The check SHALL run best-effort in the background and SHALL NOT block the service from starting. Each binary checks its own subset, configurable via environment.

#### Scenario: a present dependency is not reinstalled

- **WHEN** a dependency's probe succeeds at startup
- **THEN** it is left as-is (no install) and startup proceeds

##### Example: default dependency subsets

| Binary | Ensures |
| ------ | ------- |
| fleety-server | python (runtime), ddgs (package), node (runtime), insyra (binary) |
| fleetyd | insyra (binary); optionally python / node (runtime) |


<!-- @trace
source: startup-dependencies
updated: 2026-06-28
code:
  - crates/fleety-tools/src/deps/runtime.rs
  - crates/fleety-tools/src/deps/insyra.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - docs/env.md
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-server/src/winsvc.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/deps.rs
  - crates/fleety-server/src/builtin_mcp.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/provision.rs
-->

---
### Requirement: Missing runtimes install as managed portable, no root

When a language runtime (node or python) is missing, the service SHALL install a managed, portable copy into a fleety-owned directory and prepend its bin directory to the service process's own PATH, so child processes (MCP servers, skill scripts) can find it. It SHALL NOT require administrator/root rights and SHALL NOT modify the user's system or system PATH. Python is provisioned via `uv` (a downloaded managed binary); node via the official portable distribution.

#### Scenario: missing node is provisioned without root

- **WHEN** node is absent at startup and auto-install is enabled
- **THEN** a portable node is downloaded into the fleety runtimes directory and its bin is added to the service's PATH, without root and without changing the system

#### Scenario: child processes see the managed runtime

- **WHEN** a managed runtime has been provisioned and the service later spawns a child (e.g. an MCP server or a skill script)
- **THEN** the child inherits the service PATH and can invoke the managed runtime


<!-- @trace
source: startup-dependencies
updated: 2026-06-28
code:
  - crates/fleety-tools/src/deps/runtime.rs
  - crates/fleety-tools/src/deps/insyra.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - docs/env.md
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-server/src/winsvc.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/deps.rs
  - crates/fleety-server/src/builtin_mcp.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/provision.rs
-->

---
### Requirement: Auto-install is best-effort and configurable

Dependency auto-install SHALL be best-effort: any failure (offline, download error, unsupported platform, unwritable directory) SHALL be logged with an actionable message and SHALL NOT stop the service. Auto-install SHALL be disablable via environment so air-gapped or hermetic deployments only detect and report.

#### Scenario: a failed install does not stop startup

- **WHEN** a dependency cannot be installed (e.g. the device is offline)
- **THEN** the failure is logged with an actionable message and the service still starts

#### Scenario: disabling auto-install only detects

- **WHEN** auto-install is disabled by environment
- **THEN** missing dependencies are reported but nothing is installed, and the service starts

<!-- @trace
source: startup-dependencies
updated: 2026-06-28
code:
  - crates/fleety-tools/src/deps/runtime.rs
  - crates/fleety-tools/src/deps/insyra.rs
  - crates/fleety-tools/src/restart.rs
  - crates/fleety-daemon/src/update.rs
  - crates/fleety-tools/src/computer.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-daemon/src/backoff.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/lib.rs
  - docs/env.md
  - crates/fleety-daemon/src/winsvc.rs
  - crates/fleety-server/src/winsvc.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/deps.rs
  - crates/fleety-server/src/builtin_mcp.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/provision.rs
-->

---
### Requirement: Install provisions the insyra sidecar symmetrically

fleety-server and fleetyd SHALL both provision the fleety-insyra data-analysis sidecar as part of their install/up lifecycle command, best-effort, so the install-time behavior is symmetric across the two binaries. When provisioning fails, each binary SHALL print a console note stating that on-host data analysis (insyra_exec) will be unavailable until a later `update` succeeds. A sidecar provisioning failure SHALL NOT fail the install/up command.

#### Scenario: server up provisions the sidecar

- **WHEN** the user runs `fleety-server up` (or `install`) on a supported platform with network access
- **THEN** the fleety-insyra sidecar is provisioned next to the server executable as part of the command, matching fleetyd's install behavior

#### Scenario: sidecar provisioning failure warns but install still succeeds

- **WHEN** the sidecar cannot be provisioned during install/up (e.g. the device is offline)
- **THEN** the command prints a console note that insyra_exec will be unavailable until a later update succeeds, and the install/up command still completes successfully

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