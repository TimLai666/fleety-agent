## ADDED Requirements

### Requirement: Install provisions the insyra sidecar symmetrically

fleety-server and fleetyd SHALL both provision the fleety-insyra data-analysis sidecar as part of their install/up lifecycle command, best-effort, so the install-time behavior is symmetric across the two binaries. When provisioning fails, each binary SHALL print a console note stating that on-host data analysis (insyra_exec) will be unavailable until a later `update` succeeds. A sidecar provisioning failure SHALL NOT fail the install/up command.

#### Scenario: server up provisions the sidecar

- **WHEN** the user runs `fleety-server up` (or `install`) on a supported platform with network access
- **THEN** the fleety-insyra sidecar is provisioned next to the server executable as part of the command, matching fleetyd's install behavior

#### Scenario: sidecar provisioning failure warns but install still succeeds

- **WHEN** the sidecar cannot be provisioned during install/up (e.g. the device is offline)
- **THEN** the command prints a console note that insyra_exec will be unavailable until a later update succeeds, and the install/up command still completes successfully
