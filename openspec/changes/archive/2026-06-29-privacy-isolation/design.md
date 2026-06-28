## Context

identity-core resolves an acting user per turn but enforces nothing: conversations live under `fleet/devices/<device>/conversations/` and per-user memory isn't scoped on reads. The converged model makes the user the privacy boundary and stores conversations user-primary. This change lands that enforcement: a data-layer guard plus a storage relayout, so isolation is structural rather than prompt-dependent. It is the substrate the parked conversation-recall/lifecycle will scope on.

## Goals / Non-Goals

**Goals:**
- Conversations stored user-primary (`users/<user>/conversations/`), device recorded per event; existing data migrated once.
- All conversation/recall/memory reads scoped to the acting user via a guard; cross-user is default-deny.
- A hard no-leak rule covering content, timing, and existence; explicit cross-user grants only.
- Guest has no access to any real user's private data.

**Non-Goals:**
- No identity (identity-core). No timezone (per-user-timezone). No building recall/lifecycle (only the primitive). No encryption-at-rest. No fine-grained ACLs.

## Decisions

### User-primary conversation storage, device recorded per event

Conversation files move to `fleet/users/<user_id>/conversations/<conversation_id>.jsonl`; each event keeps the `device_id` it happened on (so "where" is preserved without making device the key). A person's history thus follows them across their devices, and a shared device naturally separates each user's conversations under their own space.

**Alternative:** keep device-primary and filter — rejected in the discussion: the privacy/ownership key is the user; device-primary makes per-user isolation a filter bolted on top rather than the structure.

### One-time migration with an id→owner index

On first run after this change, existing `devices/<device>/conversations/*` are migrated: each conversation is placed under its device's `owner` (from identity-core) if set, else under a reserved unattributed/legacy user; every migrated event is stamped with its origin `device_id`. An `conversation_id → owner` index is maintained so `Resume`/lookup by conversation id still resolves to the right user space. The migration is idempotent (skips already-migrated) and never deletes the source until the copy is verified.

**Alternative:** lazy migrate on access — rejected (resume/recall need a complete view; a one-time pass with an index is simpler to reason about and verify). **Alternative:** drop old conversations — rejected (lossless is required).

### A data-layer access guard keyed to the acting user

A `privacy` guard mediates every conversation/recall/memory read: `can_access(acting: &ActingUser, resource_owner: &str, grants) -> Decision`. Allowed when the acting user *is* the resource owner, or an explicit grant covers it; otherwise denied. Storage accessors take the acting user and return only that user's data (or grant-covered data); there is no unscoped read path used by a turn. Guest matches no resource owner and has no grants → denied for all private data.

**Alternative:** enforce only in the prompt — rejected: prompts can be talked around; the boundary must be structural at the data layer.

### No-leak covers content, timing, and existence — and refusals reveal nothing

The hard rule (policy.md + guard behavior): the agent must never disclose another user's information without that user's authorization — not the content, not when they used it, not even whether they exist or whether a topic was discussed. A denied cross-user access returns a uniform "not available to you" that does not distinguish "no such data" from "exists but forbidden", so the refusal itself leaks nothing.

**Alternative:** deny with "user X has no conversation about Y" — rejected (confirms X exists / the topic's absence; that is itself a leak).

### Cross-user access is default-deny with explicit, coarse grants

Default is deny. A user may explicitly grant another principal access to a defined scope (e.g. "share my project notes with bob"); grants are stored and consulted by the guard. Grants are coarse (per-user / per-scope), not fine-grained ACLs — enough for "let my assistant tell my partner about the shared trip" without building a permission system.

**Alternative:** open-by-default within a household — rejected (violates the privacy promise; sharing must be intentional).

### Guest gets no private data

Guest (identity-core's unidentified principal) can run stateless tasks but reads no real user's conversations/memory/recall. Any per-turn scratch a Guest produces is not attributed to a real user and is not surfaced to one.

## Implementation Contract

**Behavior:** After this change a turn can only read the acting user's conversations, recall, and per-user memory (plus anything explicitly granted); it cannot reach another user's data, and a refusal does not reveal whether that data exists. Existing conversations are migrated once under their device owner (or a legacy bucket), with device recorded per event and resume-by-id still working. Guest reads no private data. The agent is bound by a policy rule never to disclose another user's content, timing, or existence without authorization. Nothing panics; the migration is lossless and idempotent.

**Interfaces / data shapes:**
- Storage: conversation path `users/<user_id>/conversations/<id>.jsonl`; events carry `device_id`; a `conversation_id → owner` index; accessors take `&ActingUser` and return only permitted data; a one-time `migrate_conversations()` (idempotent, verify-before-delete).
- `privacy::can_access(acting: &ActingUser, resource_owner: &str, grants: &Grants) -> Decision` (Allow / Deny) — pure.
- `Grants`: a per-owner store of explicit grants (owner → {grantee, scope}); load/save; consulted by the guard.
- conn: resolve acting_user (identity-core) and pass it to every conversation/memory/recall accessor; map a Deny to a uniform non-revealing response.
- Policy: the no-leak rule (content/timing/existence) + explicit-authorization requirement in policy.md.

**Failure modes:** migration interrupted → idempotent re-run completes it; source kept until copy verified, so a crash never loses data. Unknown conversation id → uniform "not available" (no existence leak). Guard given Guest → deny all private; given owner → allow; given grantee within scope → allow, outside scope → deny. Corrupt grants file → treated as no grants (fail closed, default-deny). Never panic; never widen access on error (errors fail closed).

**Acceptance criteria:**
- Guard tests (pure): owner → Allow; different user, no grant → Deny; grantee within scope → Allow; grantee outside scope → Deny; Guest → Deny for all private resources; corrupt grants → Deny (fail closed).
- Migration tests: device conversations with an owner migrate under that user with device_id stamped; ownerless device → legacy bucket; idempotent re-run is a no-op; id→owner index resolves resume; source retained until verified.
- Scoping tests: a conversation/memory read for user A returns A's data and never B's; a cross-user attempt is denied with a response that does not distinguish absent vs forbidden.
- Policy review: policy.md states the no-leak rule (content/timing/existence) and the explicit-authorization requirement.
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.
- Live cross-device/cross-user behavior on real data is environment-dependent and manual-verify.

**Scope boundaries:**
- In: user-primary conversation storage + one-time migration + id→owner index, acting-user-scoped accessors, the privacy guard + grants store, the no-leak policy rule, non-revealing refusals, Guest denial, tests.
- Out: identity resolution (identity-core), timezone (per-user-timezone), building recall/lifecycle, encryption-at-rest, fine-grained ACLs, protocol changes.

## Risks / Trade-offs

- [storage relayout + migration is risky] → idempotent, verify-before-delete, id→owner index for resume; lossless by construction.
- [a leak via refusal wording] → uniform non-revealing "not available to you"; refusals never distinguish absent vs forbidden.
- [prompt-only enforcement is bypassable] → enforced at the data layer (guard on every read); policy.md is the secondary, human-facing statement.
- [over-broad grants] → coarse but explicit and default-deny; no implicit household sharing.
- [Guest doing useful work without data] → Guest is stateless-only by design; identifying yourself unlocks your own space.
- [errors widening access] → all guard/grant errors fail closed (deny); never widen on failure.
- [parked recall/lifecycle assume per-device] → they scope on this guard when applied (the agreed fold-in), not before.
