## ADDED Requirements

### Requirement: The server installer also installs the CLI

The server install script SHALL, after installing `fleety-server` and the sidecar, also install the `fleety` CLI binary onto the same directory using the same platform/target resolution — best-effort: a CLI download failure SHALL print an actionable note (install the CLI manually) without failing the server installation. The script's closing guidance SHALL point at `fleety init` for connecting from that host.

#### Scenario: server install lands the CLI too

- **WHEN** the server install script runs on a supported platform
- **THEN** both `fleety-server` and `fleety` end up on the install directory, and the guidance mentions `fleety init`

#### Scenario: a CLI download failure does not fail the server install

- **WHEN** the CLI asset cannot be fetched during the server install
- **THEN** the script prints how to install the CLI manually and the server installation still succeeds
