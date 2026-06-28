## Context

Timestamps store as Unix epoch/UTC (storage.rs append_history) and there is no timezone rendering: audit "5m ago" is a raw delta, and the agent is never told the current local time. `chrono-tz` is already a dependency, used by schedules.rs to evaluate cron in an IANA zone. identity-core provides a per-turn acting user, so a per-user timezone is now meaningful. This change adds a rendering layer only; stored time is untouched.

## Goals / Non-Goals

**Goals:**
- An acting-user IANA timezone, with `FLEETY_TZ` then UTC fallback.
- Render the agent's current-time and displayed timestamps in that zone; storage stays UTC.
- Reuse chrono-tz; one timezone mechanism.

**Non-Goals:**
- No change to stored timestamps. No identity (identity-core). No privacy (privacy-isolation). No general i18n.

## Decisions

### Timezone resolves acting-user, then FLEETY_TZ, then UTC

`resolve_tz(user_tz: Option<&str>, env_tz: Option<&str>) -> Tz` returns the acting user's profile timezone if valid, else `FLEETY_TZ` if set and valid, else UTC. Invalid zone strings fall through rather than erroring. Pure and unit-testable.

**Alternative:** a single global timezone — rejected (different users are in different zones; the model is per-user).

### Render-only: a format-for-user helper; storage stays UTC

A helper `format_for_user(ts_secs: u64, tz: Tz) -> String` converts a stored epoch to a human string in the resolved zone (using chrono-tz). Display surfaces — audit listings, recall results, any timestamped output — render through it. Stored values remain Unix epoch; nothing about persistence changes.

**Alternative:** store local time — rejected (epoch/UTC is the correct absolute representation; only display should localize).

### Inject "now" in the acting user's zone at turn start

At the start of a turn the agent is told the current time in the acting user's zone (e.g. a line in the assembled prompt), so it reasons about "today", "this morning", scheduling, and relative times correctly. This uses the same resolver.

**Alternative:** leave the agent to assume UTC — rejected (it would mis-handle "tonight"/"tomorrow" for non-UTC users).

## Implementation Contract

**Behavior:** For a given acting user, the agent is told the current time in that user's timezone, and timestamps it presents (audit/recall/listings) are shown in that zone; a user with no configured zone falls back to `FLEETY_TZ` then UTC. Stored timestamps are unchanged (Unix epoch). Invalid timezone configuration falls back rather than failing. Nothing panics.

**Interfaces / data shapes:**
- User profile gains an optional IANA `timezone` (read with the acting user's profile).
- `resolve_tz(user_tz: Option<&str>, env_tz: Option<&str>) -> chrono_tz::Tz` — pure, precedence user → env → UTC, invalid falls through.
- `format_for_user(ts_secs: u64, tz: Tz) -> String` — pure, epoch → zoned human string.
- conn: at turn start, compute the acting user's tz and inject the current local time into the prompt; display helpers use `format_for_user`.

**Failure modes:** invalid user tz → try FLEETY_TZ → UTC. Invalid FLEETY_TZ → UTC. No identity / Guest → FLEETY_TZ then UTC. None of these error; storage is never affected. Never panic.

**Acceptance criteria:**
- `resolve_tz` tests: valid user tz wins; invalid user tz falls to env; invalid/absent env falls to UTC; Guest/no-user uses env then UTC.
- `format_for_user` test: a known epoch renders to the expected wall-clock string in a known zone (e.g. an Asia/Taipei offset) and to UTC under the UTC fallback.
- conn/render review: the current-time injected at turn start uses the resolved zone; audit/recall display uses `format_for_user`.
- Storage-unchanged test/assertion: stored timestamps remain epoch (no localization on write).
- Content review: docs/env.md documents `FLEETY_TZ` fallback and the per-user timezone.
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.

**Scope boundaries:**
- In: acting-user timezone in the profile, `resolve_tz` + `format_for_user` helpers, current-time injection at turn start, display rendering through the helper, docs, pure tests.
- Out: changing stored timestamps, identity resolution, privacy scoping, general localization/i18n.

## Risks / Trade-offs

- [invalid tz strings] → resolver falls through to env then UTC; never errors.
- [confusion between stored vs displayed time] → storage stays UTC epoch; only display/"now" localizes; documented.
- [Guest has no profile tz] → falls back to FLEETY_TZ then UTC.
- [DST correctness] → delegated to chrono-tz (the same lib schedules already trusts for cron).
