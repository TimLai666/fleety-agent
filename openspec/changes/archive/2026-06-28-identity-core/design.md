## Context

Fleety is single-user throughout: `auth.rs` maps token→device_id (no user), `storage.rs` core_memory() reads one global `USER.md`, conversations/memory/audit live under `fleet/devices/<device>/` (no user dimension), and `device.json` (ensure_device) has id/status/mobility/site/connectors but no owner. The discussion `identity-and-multiuser-privacy` converged on a user-centric model with a hard privacy boundary. This change lands the foundation of that model — knowing *who* each turn is for — without yet enforcing isolation (privacy-isolation) or rendering times (per-user-timezone).

## Goals / Non-Goals

**Goals:**
- A per-user identity store (`users/<id>` profiles + index) and a device-ownership model (owner/users/shared).
- A pure per-turn acting_user resolution (device owner / asserted user / guest), layered on the existing per-device token, with the comms sender mapping onto the same assertion.
- The core-memory USER block becomes the acting user's profile (ME/TODO stay agent-global).

**Non-Goals:**
- No isolation/scoping enforcement, no conversation-storage relayout (privacy-isolation).
- No timezone rendering (per-user-timezone).
- No comms integration; only the assertion field a future sender maps onto.
- No PROTOCOL_VERSION bump (assertion field is additive/optional).

## Decisions

### Per-user identity store: `fleet/users/<user_id>/` + a users index

Each user gets `fleet/users/<user_id>/` holding their profile (`USER.md`) and room for future per-user state; a `fleet/users/index` (or `users.json`) lists known users. `user_id` is a validated slug (same `validate_id` rules as device_id — no slashes/`:`). The single global `USER.md` semantics move here: there is no longer one shared USER profile.

**Alternative:** keep one USER.md with sections per user — rejected (no real isolation; the file would be readable as a whole; privacy-isolation needs a per-user path to scope to).

### acting_user resolution is pure and layered on the device token

Resolution per turn: `resolve_acting_user(device: &DeviceRecord, asserted: Option<&str>) -> ActingUser` where
- a non-empty, valid `asserted` user → `User(asserted)` (used by shared devices and, later, comms senders);
- else if the device has an `owner` → `User(owner)`;
- else → `Guest`.
The per-device token (auth.rs) is unchanged and still authenticates the *device/transport*; acting_user is resolved on top and attached to the turn. `ActingUser` is `User(user_id)` or `Guest`.

**Alternative:** replace device tokens with per-user tokens — rejected (breaks the daemon transport model; device identity is still needed; layering is additive and backward compatible).

### Device ownership: `owner` / `users` / `shared` on device.json

`ensure_device` and the device record gain `owner: Option<user_id>`, `users: Vec<user_id>` (authorized), `shared: bool`. A personal device has an owner and `shared=false`; a public device has `shared=true` and possibly several `users`. These fields drive resolution and (later) authorization. Existing device.json files without them load with defaults (owner=None, users=[], shared=false) — additive and backward compatible.

**Alternative:** a separate ownership table — rejected (device.json already is the device record; co-locating is simpler and additive).

### acting_user assertion rides an additive, optional wire field

A new optional field carries an asserted user id from the client (for shared devices today, comms senders later). It is additive on the existing message (serde default/skip, no PROTOCOL_VERSION bump): older clients send nothing → resolution falls back to device owner or guest. The server never trusts the assertion for *authorization* beyond what device.users permits (authorization is privacy-isolation's job); here it only *identifies*.

**Alternative:** a dedicated login handshake — rejected for v1 (heavier; the additive field covers personal-owner, shared-asserted, and future-sender uniformly).

### Core-memory USER block = the acting user's profile

`core_memory()` keeps ME and TODO agent-global but resolves the USER block from `users/<acting_user>/USER.md`; for a Guest, the USER block is a neutral "unidentified user" placeholder with no personal data. This makes each turn's prompt carry the right person's profile. (Reading other layers — conversations/recall — is still global here; scoping them is privacy-isolation.)

**Alternative:** leave USER global until privacy-isolation — rejected (the profile is the most visible per-user surface and is cheap to scope now; it also validates the users store end to end).

### Guest is a first-class principal

When no user can be resolved (shared device, no assertion, no owner), the acting_user is `Guest`: identity exists but names no person. identity-core simply represents Guest; privacy-isolation will define what Guest may access (the intent: no private data of any real user). Representing it now keeps every downstream check total (User vs Guest), with no "unknown/None" ambiguity.

## Implementation Contract

**Behavior:** Each turn resolves an acting_user — the device owner for a personal device, an explicitly asserted user when provided (within the device's authorized users), or Guest otherwise. The agent's USER profile block is that acting user's profile (or a neutral placeholder for Guest). Devices can be marked owned/multi-user/public via device.json. Older clients and existing device.json files keep working (assertion absent → owner/guest; missing ownership fields → defaults). Nothing here yet restricts what data the agent can read across users — that is privacy-isolation — and nothing panics.

**Interfaces / data shapes:**
- `enum ActingUser { User(String), Guest }`.
- `resolve_acting_user(device_owner: Option<&str>, device_users: &[String], asserted: Option<&str>) -> ActingUser` — pure: asserted (if valid and authorized by device_users when the device is shared) → User; else owner → User; else Guest.
- Device record additive fields: `owner: Option<String>`, `users: Vec<String>`, `shared: bool` (serde defaults).
- Storage: `users/<id>/` profile read/write; users index list; `user_profile(acting_user) -> String` (Guest → neutral placeholder); validated `user_id` slug.
- Protocol: additive optional `acting_user` (or `user`) field on the user message (serde default/skip; no version bump).
- conn: resolve acting_user per turn from the connection's device record + the asserted field, and thread it (so privacy-isolation can later scope on it); core_memory uses it for the USER block.

**Failure modes:** invalid/blank asserted user → ignored, fall back to owner/guest. Asserted user not in a shared device's `users` → treated as not-authorized-here → guest (full authorization semantics are privacy-isolation; identity-core fails closed to Guest). Missing users/<id>/USER.md → created with a default like today's DEFAULT_USER. Corrupt device.json ownership fields → defaults (owner=None/shared=false). Never panic; never block a turn.

**Acceptance criteria:**
- Pure resolver tests: asserted-valid → User; asserted-blank/invalid → owner fallback; no owner + no assertion → Guest; shared device with asserted user not in `users` → Guest (fail closed).
- Storage tests: users/<id>/USER.md read/write round-trip; users index lists known users; `user_id` slug validation rejects slashes/`:`; Guest profile is a neutral placeholder (no personal data).
- Device-ownership tests: ensure_device writes owner/users/shared; an old device.json without them loads with defaults.
- Core-memory test: with an acting user, the USER block is that user's profile; with Guest, it is the neutral placeholder; ME/TODO remain global.
- Protocol test: the acting_user field round-trips and a message without it still parses (backward compatible, no version bump).
- Content review: prompts/memory.md states USER is the acting user's profile (ME/TODO agent-global).
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.

**Scope boundaries:**
- In: users/<id> profile store + index, device ownership fields, pure acting_user resolver, additive assertion field, acting-user-scoped USER core-memory block, Guest principal, identity wiring in conn/auth, prompt note, tests.
- Out: privacy enforcement / scoping of conversations/recall/memory, conversation-storage relayout, cross-user authorization rules, timezone rendering, comms integration, per-user tokens, PROTOCOL_VERSION bump.

## Risks / Trade-offs

- [identity without enforcement is a half-step] → intentional: identity-core is the foundation; privacy-isolation enforces. Apply them in sequence; identity alone changes only the USER profile surface (no new leak vs today, which is already global).
- [trusting a client-asserted user] → identity-core only *identifies*, never grants cross-user access on assertion alone; fail-closed to Guest when unauthorized; real authorization is privacy-isolation.
- [device.json / protocol additions] → all additive with serde defaults; old clients and files unaffected; no version bump.
- [Guest ambiguity] → modeled explicitly as a principal so downstream checks are total (User vs Guest), not None-handling.
- [USER profile moves per-user before isolation lands] → low risk: it only changes which profile is injected; conversations/recall stay global until privacy-isolation.
