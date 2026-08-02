## ADDED Requirements

### Requirement: The compaction budget measures only compactable history

The character budget that decides whether compaction runs SHALL measure only the portion of the context that compaction is able to compact. The leading, non-compactable preamble — the system prompt and any further system messages the caller places ahead of the conversation history — SHALL be excluded from that measurement.

Rationale, stated normatively: because the preamble is re-sent verbatim on every turn regardless of compaction, including it in the budget makes the threshold unconditionally exceeded and reduces the decision to a message-count check, which is not the intended gate.

The threshold value itself SHALL remain a fixed constant and SHALL NOT be exposed as an environment variable by this capability.

#### Scenario: a long preamble alone does not trigger compaction

- **WHEN** the leading non-compactable preamble is far larger than the threshold but the conversation history after it is smaller than the threshold
- **THEN** compaction does not run, the context is sent unchanged, and no summarization model call is made

##### Example: preamble dwarfs the threshold

- **GIVEN** a threshold of 24000 characters, a leading preamble totalling 58000 characters, and 12 history messages totalling 3000 characters
- **WHEN** the turn assembles its context
- **THEN** compaction does not run, because the compactable history measures 3000 characters

#### Scenario: history over the threshold still triggers compaction

- **WHEN** the conversation history after the preamble exceeds the threshold and there are more messages than the preamble plus the recent-keep window
- **THEN** compaction runs, summarizing the older middle and keeping the recent messages verbatim

### Requirement: The whole leading preamble survives compaction

Compaction SHALL preserve every leading system message verbatim, not only the first one. The preserved run SHALL be the maximal sequence of system messages at the head of the context. These messages SHALL NOT be folded into the summary, SHALL NOT be dropped, and SHALL appear ahead of the summary in the rebuilt context.

This requirement exists so that a caller which re-injects ephemeral per-turn context — the current time, an origin preamble, and injected instruction files — as leading system messages sees that context reach the model on every turn, as the instruction-file injection capability already requires.

#### Scenario: multiple leading system messages are all kept

- **WHEN** a context begins with several consecutive system messages and its compactable history exceeds the threshold
- **THEN** the rebuilt context contains all of those leading system messages unchanged, followed by the summary, followed by the recent messages

##### Example: five-message preamble preserved

- **GIVEN** a context of: system prompt, current-time system message, origin system message, local instruction-file system message, remote instruction-file system message, then 30 history messages over the threshold
- **WHEN** compaction runs
- **THEN** the rebuilt context is those 5 system messages, then one summary message, then the most recent history messages — and the current-time, origin, and instruction-file text appears in none of the summarized content

#### Scenario: re-injected preamble is never stale

- **WHEN** a conversation compacts on one turn and the caller re-injects a fresh preamble on the next turn
- **THEN** the model receives the fresh preamble, not preamble text captured inside an earlier summary
