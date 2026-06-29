## Context

`device_id()` (fleety-cli/fleety-daemon) returns `FLEETY_DEVICE_ID` else
`COMPUTERNAME`/`HOSTNAME`; the client sends it in Hello and the server keys the
hub (`device_id -> sender`), per-device storage (`fleet/devices/<id>/...`), and
device ownership on it. Auth already binds a token to a device id
(`AuthStore.tokens: token -> device_id`, `redeem(code, device_id)`,
`verify(token) -> device_id`) but the bound id is the self-asserted hostname.
`migrate_conversations` shows the project's verify-before-delete one-time
migration pattern. This change makes the device id machine-derived and
token-authenticated, and migrates existing hostname-keyed data losslessly.

## Goals / Non-Goals

**Goals:** a stable machine-derived id all local processes agree on; collision-free
across machines; anti-spoof via token binding when authenticated; hostname kept as
a label; lossless one-time migration of existing devices.

**Non-Goals:** un-merging already-collided hostname data; the per-connection id
(acp-editor-delegation); changing the token/pairing scheme.

## Decisions

### device_id is the OS machine id, overridable, with hostname as a label

`device_id()` reads a stable machine id via `machine-uid` (Windows MachineGuid,
Linux /etc/machine-id, macOS IOPlatformUUID). `FLEETY_DEVICE_ID` overrides it (VM/
container clones that share a machine id). The hostname is sent separately as a
display label, never as the identity key.

**Alternative:** client-persisted random uuid — rejected (daemon and `fleety acp`
on one machine would persist different uuids and not line up; a machine id is
identical for every process on the host).

### Authenticated connections resolve the id from the token, not the Hello field

`redeem` binds the token to the machine id at pairing. On an authenticated
connection the server uses `verify(token)` as the authoritative device id and
ignores the self-asserted Hello id (anti-spoof). With auth off (`full_access`
default) there is no token, so the Hello machine id is used directly — still
collision-free, just not anti-spoof.

**Alternative:** keep trusting the Hello id — rejected (self-asserted; a client
could claim another device's id).

### Hello carries an optional hostname label (additive protocol change)

Hello gains an optional `hostname` field so the server has the label at connect
time (before `ensure_device`) for `device.json` and the migration lookup. Additive:
old clients send `None`; `PROTOCOL_VERSION` is unchanged.

**Alternative:** defer the migration to the first UserMessage (which carries
`OriginContext.hostname`) — rejected (the device directory is created at connect in
`ensure_device`; doing identity/migration there needs the label at connect).

### One-time, per-device, verify-before-delete migration keyed by hostname→machine-id

On connect, before using the id, the server runs a migration: if a legacy
directory keyed by the reported hostname exists and no directory for the machine id
exists yet, move the device's data (`conversations/`, `history.jsonl`,
`device.json`) into the machine-id directory and rebind any token to the machine
id. Verify-before-delete (source removed only after the destination is written and
matches); idempotent (a migrated device is skipped). Mirrors
`migrate_conversations`.

**Alternative:** keep a permanent old-id→machine-id mapping table — rejected
(every lookup pays the indirection forever; a one-time move is simpler and final).

### Pre-existing collisions are not un-merged

Two same-hostname machines that already shared `fleet/devices/<hostname>/` before
the upgrade have intermixed data with no per-machine origin marker. The migration
moves that directory to the first machine's machine-id; the second machine then
starts clean under its own machine-id. The already-intermixed history cannot be
split. Documented, not silently glossed.

## Implementation Contract

**Behavior:** After upgrade, each machine has a unique, stable `device_id` (its OS
machine id, or `FLEETY_DEVICE_ID`); all local processes report the same one. A
connecting device's existing data (paired or not) is migrated once from its
hostname-keyed directory to its machine-id directory, losslessly, and any token is
rebound. Authenticated connections take their device id from the token (a spoofed
Hello id is ignored); unauthenticated connections use the machine id from Hello.
The hostname appears as a label in `device.json`. Nothing panics; a failed
migration leaves the source intact and is retried next connect.

**Interfaces / data shapes:**
- `device_id()` (cli + daemon): machine id via `machine-uid`, `FLEETY_DEVICE_ID`
  override, documented fallback if the machine id can't be read.
- A `hostname()`/label the client sends in Hello.
- `fleety_protocol` Hello: add `hostname: Option<String>`.
- `AuthStore`: `redeem` binds the machine id; `verify` resolves it (API shape
  unchanged; semantics: the bound id is now a machine id).
- conn: resolve the working device id from the token when authenticated, else the
  Hello machine id; call the device migration before first use.
- storage: `migrate_device(hostname, machine_id) -> Result<bool>` (verify-before-
  delete move of the device directory + token rebind hook); idempotent.

**Failure modes:** machine id unreadable → documented fallback (e.g.
`FLEETY_DEVICE_ID` required, or a clear error) — never a silent hostname collision.
Migration partial/crash → source intact, destination discarded/ignored, retried.
Both source and destination exist (already migrated or manual) → skip, no clobber.
Auth on but token invalid → rejected as today. Hostname changed since pairing →
legacy dir not found by lookup; device starts under machine-id (old data orphaned,
not lost) — documented; `FLEETY_DEVICE_ID` or manual move covers it.

**Acceptance criteria:**
- Unit: `device_id()` honors `FLEETY_DEVICE_ID` override; falls back deterministically.
- Migration tests (storage, no network): legacy `fleet/devices/<hostname>/` with
  conversations + history + device.json moves to `fleet/devices/<machine-id>/`
  intact; idempotent (second run is a no-op); destination-exists → skip; source
  absent → no-op.
- Auth tests: `redeem` binds a machine id; `verify` returns it; an authenticated
  connection's resolved id equals the token's bound id, not a differing Hello id.
- Protocol: Hello round-trips with and without the optional hostname.
- fmt + clippy --workspace -D warnings + tests green; agent-core stays host-free.
- Reading a real OS machine id and live multi-machine collision behavior are
  environment-dependent and manual-verify.

**Scope boundaries:**
- In: machine-id `device_id()` + override + label, token-authoritative resolution,
  Hello hostname field, one-time device migration, docs, the testable units above.
- Out: un-merging prior collisions, per-connection id, token/pairing redesign.

## Risks / Trade-offs

- [machine id missing/duplicated on VM/container clones] → `FLEETY_DEVICE_ID`
  override; documented.
- [hostname changed since pairing] → migration lookup misses; data orphaned not
  lost; override/manual move; documented.
- [anti-spoof only when auth is on] → the default `full_access`/no-auth mode uses
  the Hello machine id (collision-free but self-asserted); anti-spoof requires
  auth, as today.
- [additive protocol change] → optional field, old clients unaffected;
  `PROTOCOL_VERSION` unchanged.
- [new dependency `machine-uid`] → small, cross-platform; isolated to `device_id()`.
- [interaction with privacy-isolation] → ownership keyed on device id becomes
  reliable once ids are unique; the migration preserves existing ownership records
  by moving `device.json` with the directory.
- [interaction with acp-editor-delegation] → that change's per-connection id sits
  on top of this machine id (host); the two identities compose.
