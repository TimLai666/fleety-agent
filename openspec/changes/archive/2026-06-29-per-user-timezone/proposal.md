## Why

Timestamps are stored as Unix epoch (UTC), which is correct, but nothing renders them in a meaningful timezone: the agent has no notion of "now" in the user's zone, and audit/recall times are shown as raw values. With identity-core giving a per-turn acting user, each user can have their own timezone so the agent reasons and reports times in the right zone — without changing how times are stored.

## What Changes

- **Per-user timezone**: the acting user's profile carries an IANA timezone; the agent's view of "now" and any time it presents are rendered in that zone. Resolution precedence: the acting user's tz, then a global `FLEETY_TZ`, then UTC.
- **Render, don't restore**: storage stays Unix epoch (UTC). A formatting helper converts an epoch to the acting user's zone for display (audit/recall/listings) and for the current-time the agent is told at turn start.
- **Reuse chrono-tz**: the same `chrono-tz` already used for cron timezones formats the display, so there is one timezone mechanism.

## Non-Goals

- Not changing stored timestamps (they remain UTC epoch).
- Not adding identity (identity-core, the dependency for "which user's tz").
- Not the privacy boundary (privacy-isolation).
- Not localizing anything beyond time (no full i18n).

## Capabilities

### New Capabilities

- `per-user-timezone`: an acting-user IANA timezone (profile, with `FLEETY_TZ` then UTC fallback) used to render the agent's current-time and displayed timestamps, while storage stays UTC; built on chrono-tz.

### Modified Capabilities

(none in spec terms — storage is unchanged; this adds a rendering layer.)

## Impact

- Affected specs: new `per-user-timezone`. Depends on `user-identity` (identity-core) for the acting user.
- Affected code:
  - Modified: crates/fleety-server/src/storage.rs (acting user's tz in the user profile; a format-for-user helper turning epoch → the user's zone; audit/listing display uses it), crates/fleety-server/src/conn.rs (inject the current time in the acting user's zone at turn start so the agent reasons in the right zone), docs/env.md (document `FLEETY_TZ` fallback and per-user tz)
  - New: none required (a small tz helper can live in storage or a tiny module)
  - Removed: none
