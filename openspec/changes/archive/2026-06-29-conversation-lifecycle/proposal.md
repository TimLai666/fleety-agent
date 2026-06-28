## Why

Long conversations only survive today by in-context compaction, and nothing carries lessons forward: when a task is done, the conversation keeps growing and any durable takeaway is lost unless the agent happened to write it somewhere. The user wants conversations to roll over (start fresh) when a task is complete or the agent judges it useful, distilling the worthwhile takeaways into the right memory layer first — and, because conversation-recall makes old conversations searchable, rolling over is low-cost (nothing is lost, just set aside). Crucially the rollover prompting must be invisible to the user: the agent should never reply in a "the system asked me to…" voice.

## What Changes

- **Conversation rollover** (per-device): the agent can start a fresh conversation for the device while the previous one is preserved and remains searchable via conversation-recall. A successor is recorded; the active conversation switches; front-ends are told via an additive server message and otherwise continue to work.
- **Two triggers, both agent-judged**: (1) an explicit `rollover_conversation` tool the agent calls when it decides a task/topic is done; (2) an implicit, out-of-band nudge after a goal completes (and under heavy context pressure) that prompts the agent to consider distilling + rolling over. The nudge reuses the existing silent post-turn reflection path — it never produces user-facing system-speak.
- **Type-routed distillation**: before/at rollover the agent distills takeaways by *kind*, routing each to the right layer — durable knowledge/insight → wiki, pending work → TODO, user facts → USER, device-operational facts → device NOTES, ephemeral recap → nothing (conversation-recall already covers it). The wiki holds wisdom, not raw transcripts; routing is the agent's judgment, guided by prompt rules.
- **Invisible operation**: the distillation/rollover reflection runs silently (like the learning-loop reflection); the user sees normal answers, never a system-style report about housekeeping.
- **Never occupies the user's time**: all post-turn housekeeping (this distillation/rollover *and* the existing skill-learning-loop reflection, which today is awaited inline) moves to a background task off the connection loop, on the economy model tier, single-flight per conversation — so the user's next message is handled immediately instead of waiting for an extra reflection turn.

## Non-Goals

- Not auto-switching on raw length alone — length only raises the same implicit nudge; the agent decides.
- Not a new memory store — distillation writes through existing memory/wiki/device-notes tools.
- Not cross-device rollover — rollover is per-device, matching where conversations live.
- Not changing recall (this depends on conversation-recall, which provides searchability of the set-aside conversations).

## Capabilities

### New Capabilities

- `conversation-lifecycle`: per-device conversation rollover (preserving + chaining the old conversation, searchable via recall) triggered by an explicit agent tool or an implicit out-of-band nudge after goal completion / under context pressure; type-routed distillation of takeaways into wiki / TODO / USER / device-notes via existing tools; all housekeeping runs silently (no user-facing system-speak) and in the background (off the connection loop, economy tier, single-flight) so it never blocks the user's next message.

### Modified Capabilities

(none — reuses goal-completion's completion signal, the learning-loop's silent reflection path, and conversation-recall, without changing their specs.)

## Impact

- Affected specs: new `conversation-lifecycle`. Depends on the parked `conversation-recall`; reuses `goal-completion` and `skill-learning-loop` reflection.
- Affected code:
  - Modified: crates/fleety-protocol/src/lib.rs (additive `ConversationRolled` server message; backward compatible, no version bump), crates/fleety-server/src/conn.rs (rollover handling: mint successor, switch active conversation, emit ConversationRolled, transparent redirect for clients that ignore it; the silent lifecycle reflection after goal completion / under pressure), crates/fleety-server/src/storage.rs (mark a conversation ended with a successor link), crates/fleety-server/src/tools.rs (register the `rollover_conversation` tool), prompts/rules.md (distillation routing rules + rollover guidance + the invisible-housekeeping rule)
  - New: none required (logic lands in existing server modules; a small lifecycle module may be added under crates/fleety-server/src if conn.rs grows too large)
  - Removed: none
