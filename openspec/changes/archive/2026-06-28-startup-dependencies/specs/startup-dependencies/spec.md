## ADDED Requirements

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

### Requirement: Missing runtimes install as managed portable, no root

When a language runtime (node or python) is missing, the service SHALL install a managed, portable copy into a fleety-owned directory and prepend its bin directory to the service process's own PATH, so child processes (MCP servers, skill scripts) can find it. It SHALL NOT require administrator/root rights and SHALL NOT modify the user's system or system PATH. Python is provisioned via `uv` (a downloaded managed binary); node via the official portable distribution.

#### Scenario: missing node is provisioned without root

- **WHEN** node is absent at startup and auto-install is enabled
- **THEN** a portable node is downloaded into the fleety runtimes directory and its bin is added to the service's PATH, without root and without changing the system

#### Scenario: child processes see the managed runtime

- **WHEN** a managed runtime has been provisioned and the service later spawns a child (e.g. an MCP server or a skill script)
- **THEN** the child inherits the service PATH and can invoke the managed runtime

### Requirement: Auto-install is best-effort and configurable

Dependency auto-install SHALL be best-effort: any failure (offline, download error, unsupported platform, unwritable directory) SHALL be logged with an actionable message and SHALL NOT stop the service. Auto-install SHALL be disablable via environment so air-gapped or hermetic deployments only detect and report.

#### Scenario: a failed install does not stop startup

- **WHEN** a dependency cannot be installed (e.g. the device is offline)
- **THEN** the failure is logged with an actionable message and the service still starts

#### Scenario: disabling auto-install only detects

- **WHEN** auto-install is disabled by environment
- **THEN** missing dependencies are reported but nothing is installed, and the service starts
