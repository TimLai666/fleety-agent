## 1. Client installer bootstrap

- [x] 1.1 Implement **Client installation provisions the local daemon** with a network-free installer contract check that fails against the current client installers when either `fleetyd` asset or the ordered `fleetyd install`/`fleetyd start` bootstrap is absent, and passes when the Unix and Windows contracts are present; verify with `bash scripts/check-installers.sh` and `sh -n scripts/install.sh scripts/install-server.sh`.
- [x] 1.2 Implement **Decision: Client installer owns both client binaries** and the **Interface / data shape** contract so `scripts/install.sh` and `scripts/install.ps1` stage, validate, and install matching `fleety` and `fleetyd` assets; verify with the installer contract check and mocked download/extraction assertions.
- [x] 1.3 Implement **Decision: Register and start, but do not enable autostart** and the **Observable behavior** and **Failure modes** contracts so both client installers invoke the installed daemon's `install` then `start`, leave `enable` opt-in, and fail non-zero with component-specific remediation; verify with mocked service-command invocations and existing Unix syntax checks.

## 2. Release and deployment surfaces

- [x] 2.1 Implement **Release packaging exposes every client runtime asset**, **Decision: Verify installers without network or service side effects**, and the **Acceptance criteria** contract by adding a release packaging assertion that every target has non-empty `fleety`, `fleety-server`, and `fleetyd` artifacts before upload; verify with the workflow's package-check shell and `spectra analyze install-client-daemon --json`.
- [x] 2.2 Implement **Server installation documents the client boundary**, **Decision: Preserve the server installer boundary**, and the **Scope boundaries** contract by updating `scripts/install-server.sh`, `README.md`, and `docs/env.md` to distinguish server-only deployment from client daemon bootstrap and to document the no-autostart policy; verify with content review and `rg` checks for the documented commands and asset names.

## 3. Delivery and compatibility review

- [x] 3.1 Re-read the **Migration Plan**, **Risks / Trade-offs**, and **Open Questions** contract, run the complete installer checks plus focused Rust CLI/service tests, and confirm existing connection-store and pairing files are untouched; verify with `bash scripts/check-installers.sh`, `cargo test -p fleety-cli --test cli_smoke -- --test-threads=1`, and `cargo test -p fleety-daemon --test fleetyd_smoke -- --test-threads=1`.
