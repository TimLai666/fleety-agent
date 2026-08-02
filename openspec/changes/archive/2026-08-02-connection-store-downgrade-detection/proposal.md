## Why

`connections.toml` is shared by `fleety` and `fleetyd`, but the current format has no store-level compatibility marker. The per-profile generation envelope detects some field loss only after an older binary has rewritten the file, while a pre-security binary can still discard `endpoints`, `configured_url`, and `secure` without leaving durable evidence. That can make a newer binary silently lose the proof that a saved Server requires the encrypted control channel.

## What Changes

- Add a store-level compatibility and downgrade-detection contract for `connections.toml`.
- Make unknown, unsupported, or evidence-of-older-writer states fail closed before credential use, network connection, or profile mutation.
- Define explicit recovery guidance that updates all Fleety binaries and re-pairs the affected profile without sending the old credential.
- Route every persistent writer through the same validation while keeping raw URL, environment, ACP, and Doctor targets side-effect free.
- Add regressions for old-serializer rewrites, unrelated healthy profiles, atomic `0600` writes, and zero credential exposure on rejection.

## Non-Goals

- Do not promise that a legacy binary can preserve fields it does not understand.
- Do not redesign the existing per-profile generation envelope or the authenticated candidate handshake.
- Do not add a migration that guesses a lost configured endpoint or silently restores a secure latch.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `connection-profiles`: add store-level compatibility validation and downgrade recovery before durable profile use.

## Impact

- Affected specs: `connection-profiles`
- Affected code:
  - Modified: `crates/fleety-tools/src/connection.rs`
  - Modified: `crates/fleety-cli/src/main.rs`
  - Modified: `crates/fleety-cli/src/server.rs`
  - Modified: `crates/fleety-cli/src/config_panel.rs`
  - Modified: `crates/fleety-daemon/src/main.rs`
  - Modified: `crates/fleety-cli/tests/cli_smoke.rs`
  - Modified: `crates/fleety-daemon/tests/fleetyd_smoke.rs`
  - Modified: `docs/design-cli-config.md`
  - Modified: `docs/env.md`
  - Modified: `README.md`
