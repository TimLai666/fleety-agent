## Context

Conversations grow without bound and only in-context compaction (agent-core) keeps them sendable; nothing distills lessons or starts a fresh thread. Fleety already has the pieces this builds on: goal-completion emits a completion signal (`complete_goal`), the skill-learning-loop runs a silent out-of-band post-turn reflection (conn.rs `maybe_reflect`), conversation-recall (the prerequisite change) makes set-aside conversations searchable with time, and memory/wiki/device-notes tools exist for writing distilled facts. This change wires those into a rollover + distillation lifecycle that is per-device and invisible to the user.

## Goals / Non-Goals

**Goals:**
- Per-device conversation rollover that preserves and chains the old conversation (searchable via recall).
- Agent-judged triggers: an explicit tool, and an implicit out-of-band nudge after goal completion / under context pressure.
- Type-routed distillation into the right layer (wiki / TODO / USER / device-notes / nothing).
- All housekeeping runs silently — no user-facing system-speak.

**Non-Goals:**
- No length-only auto-switch (length only nudges; the agent decides).
- No new memory store (distill via existing tools).
- No cross-device rollover.
- No recall changes (depends on conversation-recall).

## Decisions

### Rollover mints a successor; old conversation is preserved and chained

`rollover` creates a new conversation id for the device and marks the current one ended with a `successor` link (stored). The active conversation for the connection switches to the successor; the old conversation's JSONL stays intact and searchable via conversation-recall. Rollover never deletes anything — it sets aside.

**Alternative:** archive/delete the old conversation — rejected (recall depends on it; rollover must be lossless).

### Client is told via an additive `ConversationRolled` message, with transparent redirect as fallback

The server emits an additive `ConversationRolled { old, new }` server message so front-ends (CLI, ACP adapter) switch their active conversation id; PROTOCOL_VERSION is unchanged and the field is serde-skippable, so older clients ignore it. For a client that ignores it and keeps sending the old id, the server **transparently redirects** appends to the successor, so behavior stays correct even without client support.

**Alternative:** require clients to handle a new id — rejected (would break older clients); transparent redirect keeps it backward compatible.

### Two triggers, both agent-judged; the implicit one is silent

- **Explicit:** a `rollover_conversation { distill?, note? }` tool the agent calls when it judges a task/topic done.
- **Implicit:** after `complete_goal` fires (and when context pressure is high — e.g. compaction has run), the server runs an out-of-band reflection that *asks the agent* whether to distill + roll over. This reuses the learning-loop's silent reflection path (`maybe_reflect`-style): the reflection turn's output is consumed internally, not streamed to the user. Length never forces a switch; it only raises this nudge.

**Alternative:** auto-rollover on goal completion without agent judgment — rejected (the agent should decide; sometimes the next task continues the same thread). Auto-switch on length — rejected (user wants agent judgment, not a hard cut).

### Distillation is type-routed by the agent, through existing tools

The distillation step routes each takeaway by kind: durable knowledge/insight → `wiki_write`; pending work → TODO (memory tools); user facts → USER; device-operational facts → device NOTES; ephemeral recap → nothing (recall covers it). The wiki holds wisdom, not transcripts. There is no new store or schema; the lifecycle supplies the trigger and prompt rules, and the agent uses the existing memory/wiki/device-notes tools. Routing rules live in prompts/rules.md.

**Alternative:** auto-summarize the whole conversation into the wiki — rejected (the user was explicit: wiki is for wisdom, not summaries; route by type).

### Invisible housekeeping (no system-speak)

The implicit reflection and any distillation/rollover it triggers run out-of-band; the user never sees the agent answer in a "the system asked me to roll over" voice. A prompt rule states this explicitly, and the reflection output is not emitted as an assistant turn to the user (same discipline as the learning-loop reflection). The user just sees normal answers; the housekeeping is silent.

**Alternative:** tell the user "starting a new conversation, here's what I saved" — rejected (the user explicitly does not want system-style narration).

### Housekeeping runs in the background, off the user's interactive path

Today the learning-loop reflection is awaited inline in the connection loop (conn.rs: `drive_to_goal` then `maybe_reflect().await`), using the main model — so although the user already has the answer, the next message they send waits for the whole reflection turn. That is unacceptable for distillation + rollover too. This change moves post-turn housekeeping **off the connection's message loop**: after the reply is emitted, the reflection/distillation/rollover runs in a spawned background task so the loop returns immediately to handle the user's next message. Properties:
- **Background spawn** (not inline await) — the user's next turn is never blocked by housekeeping.
- **Economy tier** — housekeeping uses the cheap provider (`ProviderTiers` cheap, `FLEETY_CHEAP_MODEL_*`), not the flagship main model; cost/time of an extra turn stays low.
- **Single-flight per conversation** — at most one background housekeeping per conversation; if one is in flight when a new user turn arrives (or another would start), the stale one yields/skips so tasks don't pile up.
- **Concurrency safety** — housekeeping reads a snapshot of the conversation; conversation appends are already seq-locked; rollover switches the active conversation atomically via the successor link, so a background distill can't corrupt or race the live next turn.
- The existing learning-loop skill reflection moves onto this same background runner (the user asked about *both* skill generation and memory distillation occupying their time).

**Alternative:** keep it inline-await (today's behavior) — rejected (blocks the user's next message). **Alternative:** run on the main model in the background — allowed but defaulted to economy to cut cost; main is a config fallback.

## Implementation Contract

**Behavior:** When the agent calls the rollover tool, or when (silently) it decides to after a goal completes / under pressure, the current per-device conversation is set aside (preserved + chained), a fresh conversation becomes active, and worthwhile takeaways are written to the appropriate memory layer by kind. The old conversation remains searchable via recall. Front-ends learn of the switch via an additive message; clients that ignore it still work (transparent redirect). None of this produces user-facing system narration. Nothing is deleted; nothing blocks or crashes a turn.

**Interfaces / data shapes:**
- Protocol: additive `ServerMsg::ConversationRolled { old: String, new: String }` (serde skip/default; no PROTOCOL_VERSION bump).
- Storage: a conversation can be marked ended with a `successor` conversation id; an accessor to resolve a conversation's active successor (for transparent redirect).
- Tool: `rollover_conversation { distill?: bool, note?: Option<String> }` (registered for the agent) → mints successor, emits ConversationRolled, returns the new id.
- Lifecycle reflection: an out-of-band step (reusing the learning-loop reflection mechanism) invoked after `complete_goal` and when compaction pressure is high; its prompt asks the agent to distill (type-routed) and decide on rollover; its output is not surfaced to the user.
- Background housekeeping runner: a spawned task (not inline await) carrying the reflection/distill/rollover, built on the economy tier, with a single-flight guard keyed per conversation (e.g. a per-conversation in-flight set). The existing `maybe_reflect` call site moves from `.await` in the loop to this runner.
- Prompts: distillation routing rules + rollover guidance + the invisible-housekeeping rule in rules.md.

**Failure modes:** distillation tool write fails → logged, rollover still proceeds (don't lose the set-aside). ConversationRolled emit fails / client ignores it → transparent redirect keeps appends correct. Rollover requested with no active conversation → no-op with a clear internal note. Reflection model call fails → skip silently (no user impact), conversation continues. Successor link write fails → log, keep current conversation. Never surface housekeeping to the user; never block/crash a turn.

**Acceptance criteria:**
- Protocol round-trip test: `ConversationRolled` serializes/deserializes; an older message stream without it still parses (backward compatible).
- Storage test: marking a conversation ended with a successor persists; resolving the active successor returns it; transparent-redirect resolution chains through.
- Tool test: `rollover_conversation` mints a new id, records the successor, and reports the new id; old conversation remains loadable (and thus recall-able).
- Trigger test: the implicit lifecycle reflection is invoked after a goal completes (with an injectable goal-complete signal) and its output is not emitted as a user-facing assistant turn (silent).
- Non-blocking test: after a turn, the connection loop returns to handle the next user message without awaiting housekeeping (e.g. a follow-up message is processed while a slow injectable housekeeping task is still running); the housekeeping uses the economy tier; single-flight skips a second concurrent housekeeping for the same conversation.
- Routing review: prompts/rules.md states the type-routing (wiki=wisdom / TODO / USER / device-notes / ephemeral→none) and the invisible-housekeeping rule.
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.
- Live end-to-end (a real goal completion silently distilling + rolling over across a model turn) is environment-dependent and manual-verify.

**Scope boundaries:**
- In: rollover (successor chaining + transparent redirect), additive ConversationRolled, `rollover_conversation` tool, the implicit silent lifecycle reflection hooked to goal completion / pressure, type-routed distillation via existing tools, **moving post-turn housekeeping (incl. the existing skill-learning-loop reflection) onto a background, economy-tier, single-flight runner so it never blocks the user's next message**, prompt rules, protocol/storage/tool/trigger/non-blocking tests.
- Out: recall (separate change), deleting/archiving conversations, cross-device rollover, a new memory store, length-only auto-switch, changes to the *specs* of goal-completion/learning-loop/recall (this change only changes *how* reflection is dispatched — background vs inline — not what it does).

## Risks / Trade-offs

- [client doesn't handle ConversationRolled] → transparent server-side redirect keeps it correct; the message is an optimization for UIs that want to show the switch.
- [implicit nudge leaking as system-speak to the user] → reflection runs out-of-band and its output isn't surfaced; an explicit prompt rule forbids system narration.
- [distillation dumping noise into wiki] → type-routing rule (wiki=wisdom only); ephemeral recap stays in recall, not memory.
- [over-eager rollover fragmenting a task] → rollover is agent-judged, not forced; length only nudges.
- [protocol addition] → additive + serde-skippable; no version bump; old clients unaffected.
- [depends on conversation-recall] → without it, set-aside conversations aren't searchable; apply recall first (stated dependency).
- [housekeeping occupying the user's time] → moved off the connection loop to a background economy-tier runner; the user's next message is never blocked; single-flight stops pile-up.
- [background task racing the live next turn] → housekeeping works on a conversation snapshot; appends are seq-locked; rollover switches atomically via the successor link.
- [background failures going unnoticed] → logged; never surfaced to the user and never affect the live turn (best-effort, like the existing reflection).
