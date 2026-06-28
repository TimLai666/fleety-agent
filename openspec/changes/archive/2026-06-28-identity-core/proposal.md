## Why

Fleety is built single-user: auth binds to a device, `USER.md` is one global file, conversations/memory/audit are all per-device, and a device has no notion of whose it is. To become a multi-user, privacy-aware assistant it first needs an identity layer — a way to know *who* it is talking to on each turn — before any isolation can be enforced. This change establishes that "who"; the actual privacy enforcement (scoping + no-leak) is the dependent `privacy-isolation` change.

## What Changes

- **Per-user identity store**: `fleet/users/<user_id>/` with a per-user profile (the `USER.md` that today is a single global file becomes per-user), plus a users index. The core-memory `USER` block injected each turn becomes the **acting user's** profile (ME/TODO stay agent-global).
- **Device ownership**: `device.json` gains `owner` (a user id), `users` (authorized user ids), and `shared` (bool), so a device can be personal, multi-user, or public.
- **acting_user resolution** (pure, per turn): personal device → its `owner`; an explicitly asserted user (carried in an additive, backward-compatible wire field) → that user; otherwise **guest** (an unidentified principal). The future comms-sender identity maps onto the same assertion field.
- **Auth gains a user layer**: the per-device token stays (the transport's device identity); on top, the resolved acting_user is attached to the turn. device→owner/users associations live in `device.json`.
- This change **introduces and records identity** but does **not** yet enforce isolation or move conversation storage — that is `privacy-isolation`. It also does not render times — that is `per-user-timezone`.

## Non-Goals

- Not enforcing the privacy boundary / scoping memory/conversations/recall to the acting user (that is `privacy-isolation`).
- Not moving conversation storage to user-primary (that is `privacy-isolation`).
- Not per-user timezone rendering (that is `per-user-timezone`).
- Not building the comms integration; only leaving the acting_user assertion field that a future sender maps onto.
- Not changing PROTOCOL_VERSION (the acting_user assertion field is additive/optional).

## Capabilities

### New Capabilities

- `user-identity`: a per-user identity store (`users/<id>` profiles + index), device ownership fields (owner/users/shared), and a per-turn acting_user resolution (device owner / asserted user / guest) layered on the existing per-device token — the foundation other changes scope and enforce against.

### Modified Capabilities

(none in spec terms — the single global USER profile becomes per-user, but isolation/enforcement lands in privacy-isolation.)

## Impact

- Affected specs: new `user-identity`. Foundation for the dependent `privacy-isolation` and `per-user-timezone` changes; the parked `conversation-recall`/`conversation-lifecycle` will later fold in per-user scope.
- Affected code:
  - Modified: crates/fleety-server/src/storage.rs (per-user `users/<id>/` profiles + index; the core-memory USER block reads the acting user's profile; device.json owner/users/shared on ensure_device), crates/fleety-server/src/auth.rs (attach resolved acting_user to a turn; device→users association), crates/fleety-server/src/conn.rs (resolve acting_user per turn and thread it), crates/fleety-protocol/src/lib.rs (additive optional acting_user assertion field; backward compatible, no version bump), prompts/memory.md (USER is the acting user's profile; ME/TODO stay agent-global)
  - New: crates/fleety-server/src/identity.rs (the acting_user resolver, user-id validation/slug, device-ownership parsing, users index)
  - Removed: none
