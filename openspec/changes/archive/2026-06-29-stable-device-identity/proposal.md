## Summary

Make a device's identity a stable, machine-derived, authenticated id instead of
a client-asserted hostname, so two same-named machines no longer collide and a
client cannot impersonate another device.

## Motivation

`device_id` is whatever the client asserts at Hello — `FLEETY_DEVICE_ID`, else
`COMPUTERNAME`/`HOSTNAME` (fleety-cli/fleety-daemon `device_id()`). Two machines
with the same hostname therefore get the **same** `device_id`, and the server
keys everything on it:

- the hub is `device_id -> sender`, so the later connection **overwrites** the
  earlier — tool routing can hit the wrong machine;
- per-device storage merges: `fleet/devices/<id>/` conversations, `history.jsonl`,
  and `device.json` from two machines pile into one directory;
- privacy-isolation keys device **ownership** on `device_id`, so a collision
  corrupts ownership and the isolation decision — a cross-machine leak;
- it is **self-asserted**, so a client can claim another device's id.

A stable machine id makes every local process (daemon, CLI, `fleety acp`) derive
the **same** id independently (so they line up), is unique across machines, and —
bound to the auth token — cannot be spoofed.

## Proposed Solution

- **Machine-derived id.** `device_id()` reads a stable OS machine id (Windows
  `MachineGuid`, Linux `/etc/machine-id`, macOS `IOPlatformUUID`; via the
  `machine-uid` crate). `FLEETY_DEVICE_ID` stays as an explicit override (for VM /
  container clones that share a machine id). The hostname becomes a human label,
  not the identity.
- **Authenticated identity.** Pairing (`redeem`) binds the token to the
  machine id. When a connection is authenticated, the server resolves the device
  id from the **token** (`verify`), not the self-asserted Hello field, so it can't
  be spoofed. When auth is off (the `full_access` default) the machine id from
  Hello is used directly (still collision-free, just not anti-spoof).
- **Hostname as a label.** Hello gains an optional `hostname` field (additive; old
  clients send none) so the server has the label at connect time for `device.json`
  and for the migration lookup.
- **One-time, lossless migration.** On connect, before using the id, the server
  runs a per-device, verify-before-delete migration (mirroring
  `migrate_conversations`): if a legacy directory keyed by the reported hostname
  exists and no directory for the machine id does, move the device's data
  (`conversations/`, `history.jsonl`, `device.json`) to the machine-id directory
  and rebind any token. Idempotent; a crash never loses data.

## Non-Goals

- Not un-merging data from machines that **already** collided under one hostname
  (the events carry no per-machine origin to split on); they diverge cleanly
  going forward only.
- Not the per-connection id used to tell apart multiple connections on one machine
  (that belongs to acp-editor-delegation); this change is the machine identity.
- Not changing the token/pairing scheme itself — only what id a token binds to and
  how the connection's id is resolved.

## Alternatives Considered

- **Server-minted random uuid persisted by the client** — rejected: different
  processes on one machine (daemon vs `fleety acp`) would each persist their own
  and fail to line up; a machine-derived id is the same for all of them.
- **Keep hostname, require `FLEETY_DEVICE_ID` to disambiguate** — rejected: manual,
  error-prone, and unprotected in the default no-auth case.

## Impact

- Affected specs: new `stable-device-identity`; modified `device-registry-and-routing`.
- Affected code:
  - Modified: crates/fleety-cli/src/main.rs (`device_id()` → machine id + send hostname label), crates/fleety-daemon/src/main.rs (same), crates/fleety-protocol/src/lib.rs (Hello gains optional hostname), crates/fleety-server/src/auth.rs (bind/resolve the machine id), crates/fleety-server/src/conn.rs (resolve id from token when authed; run the on-connect device migration), crates/fleety-server/src/storage.rs (one-time hostname→machine-id device migration, verify-before-delete), docs/env.md
  - New: none required (reuse existing auth + storage patterns)
  - Removed: none
  - Dependencies: add `machine-uid` (reads the OS machine id cross-platform)
