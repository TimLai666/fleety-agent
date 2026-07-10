## MODIFIED Requirements

### Requirement: Sidecar and install paths

The runtime SHALL read `FLEETY_INSYRA_BIN` for the path to the `fleety-insyra` Go sidecar (default: beside the executable) and `FLEETY_INSYRA_URL` to override its download URL for install/update. `FLEETY_INSTALL_DIR` SHALL set where the server install script lands the binary; when unset the script SHALL install to `/usr/local/bin` only when it can actually create a file there — verified by an atomic write probe (create then remove a temporary file), not a bare `-w` test that misreports an absent or root-owned directory — otherwise falling back to `$HOME/.local/bin`. Whichever directory is chosen, when it is not on `PATH` the script SHALL warn and print how to add it.

#### Scenario: sidecar resolves beside the executable

- **WHEN** `FLEETY_INSYRA_BIN` is unset
- **THEN** the `insyra_exec` tool spawns the `fleety-insyra` binary located beside the running executable

#### Scenario: install falls back when /usr/local/bin is not truly writable

- **WHEN** the install script runs without `FLEETY_INSTALL_DIR` and `/usr/local/bin` cannot actually be written (it is root-owned or absent)
- **THEN** the script installs into `$HOME/.local/bin`, and when that directory is not on `PATH` it warns and prints how to add it
