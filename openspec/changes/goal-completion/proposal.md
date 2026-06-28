## Why

In Fleety's use cases the agent should finish the whole request in one go and only then stop — not stop halfway to ask "shall I do the next step?". Today a turn ends whenever the model emits a final reply with no tool calls, so the model can hand back prematurely. We want a built-in, always-on goal mechanism: the agent sets its own goal (from the user's request + context) and a drive-to-goal loop keeps re-engaging it until the goal is met, stopping mid-way only for a genuine must-ask question. Claude Code achieves "don't stop early" softly (prompt + a self-maintained todo list + an optional stop hook); Fleety makes it explicit and reliable with clear completion signals — which fits the hard "finish it" requirement and avoids betting on the model to self-judge (the snowball risk).

## What Changes

- Always-on goal mechanism (no mode to toggle). New tools: `set_goal({goal, steps?})` (the agent self-sets its goal plus an optional self-managed checklist — the TodoWrite idea), `complete_step({step})`, `goal_status()`, `complete_goal({summary?})`, and `ask_user({question})`.
- A drive-to-goal loop: after a turn ends, if a goal is active and not yet completed and the agent did not call `ask_user`, the runtime injects a continuation nudge (the goal + pending checklist steps) and runs another turn — until `complete_goal`, `ask_user`, or a safety cap.
- Premature-stop detection is by explicit signal: a turn that ends without `complete_goal`/`ask_user` while a goal is active is treated as premature and continued. No goal set → the turn ends normally (single-shot).
- Safety cap on auto-continues (`FLEETY_GOAL_MAX_CONTINUES`) so the loop can never run away / burn unbounded tokens.
- Emission + voice gating falls out: intermediate continuation turns do not emit a terminal reply; only `complete_goal`/`ask_user` produce the user-facing reply, and (when voice mode is on) the spoken summary — so voice fires only at goal completion or a required question.

## Non-Goals

- Not a user-toggled mode — it is built in and always available.
- No external goal source required (the user may still state a goal; primarily the agent self-sets it).
- Not changing how a single turn's tool loop works (`run_turn` is unchanged); the loop wraps turns, it does not replace them.
- Not implementing text-to-speech itself (voice remains terminal-side; we only gate WHEN the speech channel text is produced).
- No automatic goal inference without the agent calling `set_goal` — engaging the loop is the agent's explicit act.

## Capabilities

### New Capabilities

- `goal-completion`: an always-on goal mechanism — the agent self-sets a goal and an optional checklist, and a drive-to-goal loop keeps re-engaging it until `complete_goal` or `ask_user`, bounded by a safety cap; intermediate continuations are silent so the user-facing reply and the spoken summary appear only at the goal's completion or a required question. Generic goal state + tools live in agent-core; the loop and emission/voice gating live in the server.

### Modified Capabilities

(none)

## Impact

- Affected specs: new capability goal-completion. Modified: none.
- Affected code:
  - New: crates/agent-core/src/goal.rs (GoalState, the five goal tools, register_goal_tools, and the premature-stop / continuation-nudge helpers), exported from the agent-core lib
  - Modified: crates/fleety-server/src/conn.rs (per-message goal state, the drive-to-goal loop wrapping the turn, terminal-only emission and voice gating), crates/fleety-server/src/storage.rs or main for the FLEETY_GOAL_MAX_CONTINUES read, docs/env.md (the cap variable), docs/tools.md (the goal tools), prompts/protocol.md and prompts/rules.md (set a goal and drive it to completion; speak only at completion or a required question)
  - Removed: none
- Key acceptance: agent-core still depends on no fleety crate; an active unmet goal auto-continues, `complete_goal`/`ask_user` stop it, the cap bounds it; workspace clippy -D and tests stay green.
