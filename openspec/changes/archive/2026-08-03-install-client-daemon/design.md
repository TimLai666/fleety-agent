## Context

The public macOS/Linux installer is a single-binary script: it downloads `fleety-<target>.tar.gz` and moves only `fleety` into the install directory. The Windows installer has the same shape. Release packaging already publishes separate `fleety`, `fleety-server`, and `fleetyd` assets, and the CLI exposes `fleety daemon <verb>` as a forwarder to the local daemon. The result is a client deployment that can install the control surface but cannot install the runtime that the Settings Daemon region and device execution require.

The daemon service contract already exists: `fleetyd install` registers the OS service, `fleetyd start` runs it now, and `fleetyd enable` controls login autostart. On Windows service installation requires an elevated terminal. The installer must not silently weaken that platform requirement.

## Goals / Non-Goals

**Goals:**

- Make the client installer provision the matching `fleety` and `fleetyd` release assets for macOS, Linux, and Windows.
- Register and start `fleetyd` after both binaries are installed.
- Keep daemon login autostart disabled by default, matching the existing service policy; users can enable it explicitly.
- Make rerunning the installer safe and make download, extraction, service-registration, and service-start failures visible.
- Keep the server installer a server deployment path and explain that it is not the client-daemon bootstrap.
- Add deterministic installer and release-asset checks that do not require network access or a running service.

**Non-Goals:**

- No changes to connection profiles, pairing, protocol frames, daemon reconnect behavior, or service-manager semantics.
- No automatic pairing, token creation, or credential migration during installation.
- No forced login autostart enablement.
- No requirement that `install-server.sh` install `fleetyd` on a server-only host.

## Decisions

### Decision: Client installer owns both client binaries

`install.sh` and `install.ps1` SHALL download the `fleety` and `fleetyd` assets for the same release target. The scripts SHALL stage and validate both extracted executables before placing them in the selected directory, so a missing daemon asset fails before a partial client bootstrap is reported as successful. The installed filenames remain `fleety`/`fleetyd` on Unix and `fleety.exe`/`fleetyd.exe` on Windows.

**Alternative rejected:** Keep downloading only `fleety` and document a separate daemon download. The repository has no supported client-side daemon download path, and it leaves the first-run Settings experience broken.

### Decision: Register and start, but do not enable autostart

After installing both binaries, the client installer SHALL invoke the daemon's existing service verbs through the absolute installed path: `fleetyd install` followed by `fleetyd start`. It SHALL NOT invoke `fleetyd enable`, because the service contract keeps login autostart opt-in. A successful installer therefore leaves the daemon running for the current session without changing boot/login policy.

**Alternative rejected:** Install the binary only. That still leaves the daemon unavailable until users discover and run a second command. **Alternative rejected:** Enable autostart unconditionally. That changes persistent OS behavior without an explicit user choice.

### Decision: Preserve the server installer boundary

`install-server.sh` SHALL continue installing `fleety-server`, its existing sidecar behavior, and the CLI helper. It SHALL state that a separate client device uses `install.sh` for both `fleety` and `fleetyd`; it SHALL not silently add a daemon to server-only deployments.

### Decision: Verify installers without network or service side effects

A repository check script SHALL inspect the installer contracts and assert that Unix and Windows client installers reference both client assets and both service bootstrap verbs. The release workflow SHALL verify that each target's packaged `fleety`, `fleety-server`, and `fleetyd` artifacts exist and are non-empty before upload. These checks complement, rather than replace, shell syntax checks and focused runtime tests.

## Implementation Contract

### Observable behavior

- Running the documented macOS/Linux client installer downloads the matching `fleety` and `fleetyd` archives, installs both into the selected directory, registers the daemon service, and starts the daemon.
- Running the documented Windows client installer downloads both zip assets, installs both executables into the selected directory, registers the daemon service, and starts the daemon.
- The installer reports which component or service step failed and exits non-zero when it cannot complete the bootstrap. It MUST NOT print success after only installing `fleety`.
- The installer does not enable login autostart and does not pair or write connection credentials.
- The server installer remains valid for server-only deployment and points client operators to the client installer for `fleetyd`.

### Interface / data shape

- Unix release assets: `fleety-<target>.tar.gz` and `fleetyd-<target>.tar.gz`.
- Windows release assets: `fleety-<target>.zip` and `fleetyd-<target>.zip`.
- Daemon service bootstrap: `<installed-dir>/fleetyd install` followed by `<installed-dir>/fleetyd start` on Unix, and the equivalent `fleetyd.exe` invocations on Windows.
- Autostart remains controlled separately by the existing `fleetyd enable` command.

### Failure modes

- A missing or failed daemon asset download, archive extraction failure, missing executable, service-registration failure, or service-start failure exits non-zero with an actionable component-specific message.
- A Windows service-registration failure caused by missing Administrator rights is surfaced as an elevation requirement, not hidden as a successful install.
- Re-running the installer replaces the matching binaries and reuses the existing service registration without creating a second service instance.
- Pairing failures after installation remain pairing failures; installation does not consume or synthesize credentials.

### Acceptance criteria

- A mocked Unix installer test proves both assets are requested, both binaries are installed, and `fleetyd install` then `fleetyd start` are invoked in order.
- A mocked Windows installer contract check proves both zip assets and both service verbs are present.
- The repository installer check passes on CI without network access.
- The release packaging job fails before upload if any of the three per-target binary archives is absent or empty.
- Existing Rust tests and shell syntax checks pass.

### Scope boundaries

In scope: `scripts/install.sh`, `scripts/install.ps1`, client-install documentation, the server-installer guidance, installer-focused checks, and release artifact checks. Out of scope: daemon internals, service-manager implementation, connection-store migration, pairing behavior, and server deployment semantics.

## Risks / Trade-offs

- [Service start can fail because of permissions or a stale service registration] → fail with the exact component and remediation; do not claim the client is ready.
- [A client install starts a background process before pairing] → start is required for the advertised local runtime, while no credential is created and autostart remains disabled; the daemon can wait for a later explicit pairing.
- [Release asset naming drifts from installer assumptions] → package-time checks and installer contract checks use the same binary/target matrix.
- [Windows elevation interrupts a one-line install] → report the Administrator requirement and preserve the installed binaries so the user can rerun only the service bootstrap.

## Migration Plan

- Release the installer and docs changes together with a version that includes all three platform binaries.
- Existing users can rerun the client installer; it installs `fleetyd`, registers the service, and starts it without changing their connection store.
- If automatic service bootstrap fails, users can run the documented `fleetyd install` and `fleetyd start` commands after resolving permissions.
- Rollback is the prior installer release; it may update only `fleety`, but it will not remove an already registered daemon service.

## Open Questions

- None for this implementation. Login autostart remains an explicit follow-up choice through `fleetyd enable`.
