## Why

The public client installer downloads only `fleety`, although the installed CLI exposes the daemon settings surface and the documented device workflow depends on `fleetyd`. A fresh client therefore reaches Settings with an unavailable Daemon and has no supported first-install path for the daemon binary.

## What Changes

- Make the macOS/Linux client installer download and install both `fleety` and `fleetyd` for the same release target.
- Register and start the installed daemon as part of the client bootstrap, while keeping login autostart an explicit policy choice.
- Bring the Windows installer and installer-facing documentation into the same client-component contract.
- Add shell/install verification that catches a client installer which publishes `fleety` without `fleetyd` or uses an unavailable release asset.
- Preserve the server installer as a server deployment path, but make its relationship to the client daemon explicit.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `service-lifecycle`: a supported client installation must provision the `fleetyd` binary before invoking its service lifecycle commands.

## Impact

- Affected specs: `service-lifecycle`
- Affected code:
  - Modified: `scripts/install.sh`
  - Modified: `scripts/install.ps1`
  - Modified: `scripts/install-server.sh`
  - Modified: `README.md`
  - Modified: `docs/env.md`
  - Modified: release/install verification scripts and workflow files that enumerate binary assets
  - New or modified: installer-focused tests
- No protocol, connection-store, or credential format changes.
