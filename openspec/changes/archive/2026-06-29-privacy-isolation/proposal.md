## Why

identity-core lets Fleety know *who* each turn is for, but nothing yet stops it from reading or revealing one user's data to another — conversations and memory are still global/per-device. The privacy promise ("never tell B anything about A without A's permission — including what was discussed, when, or even that A exists") has to be enforced at the data layer, not just asked for in a prompt. This change makes the acting user the privacy boundary and relays conversation storage to be user-primary so isolation is structural.

## What Changes

- **User-primary conversation storage**: conversations move from `fleet/devices/<device>/conversations/` to `fleet/users/<user>/conversations/`, with each event recording the `device_id` where it happened. A one-time migration moves existing conversations under their device's owner (or a reserved unattributed bucket when a device has no owner), keeping resume-by-id working via an id→owner index.
- **Acting-user-scoped reads**: every read of conversations, recall, and per-user memory goes through a guard keyed to the acting user; a turn cannot read another user's data. Guest gets no access to any real user's private data.
- **Hard no-leak rule**: a privacy rule (enforced at the data layer and stated in policy) that the agent must never disclose another user's information — content, timing, or even existence/"have we talked" — without that user's explicit authorization.
- **Cross-user authorization**: default-deny; a user can explicitly grant another principal access to a defined scope; without a grant, cross-user access is refused and the refusal itself reveals nothing about whether such data exists.
- **Foundation for the parked recall/lifecycle**: those changes, when applied, build their per-user scope on this guard rather than the old per-device scope.

## Non-Goals

- Not adding identity itself (that is identity-core, the dependency).
- Not timezone rendering (per-user-timezone).
- Not building conversation-recall/lifecycle here; only providing the scoping primitive they will use.
- Not encryption-at-rest; isolation is by access scope + file layout, same storage posture as today.
- Not a full ACL system; cross-user sharing is an explicit, coarse grant, not fine-grained permissions.

## Capabilities

### New Capabilities

- `privacy-isolation`: the acting user as a hard privacy boundary — user-primary conversation storage (device recorded per event) with a one-time migration, acting-user-scoped reads of conversations/recall/memory through a data-layer guard, a no-leak rule covering content/timing/existence, default-deny cross-user access with explicit grants, and no private-data access for Guest.

### Modified Capabilities

(none in spec terms — conversation storage relayout and scoping are new enforcement; existing capabilities' specs are unchanged.)

## Impact

- Affected specs: new `privacy-isolation`. Depends on `user-identity` (identity-core). The parked `conversation-recall`/`conversation-lifecycle` will scope on this guard when applied.
- Affected code:
  - Modified: crates/fleety-server/src/storage.rs (conversation paths become user-primary; event records device_id; id→owner index; acting-user-scoped accessors; one-time migration of existing device conversations), crates/fleety-server/src/conn.rs (thread acting_user into every conversation/memory read; refuse cross-user), prompts/policy.md (the no-leak hard rule incl. existence/timing; cross-user requires explicit authorization)
  - New: crates/fleety-server/src/privacy.rs (the access guard: given acting_user + a target resource owner, allow/deny via ownership or an explicit grant; the grant store)
  - Removed: none
