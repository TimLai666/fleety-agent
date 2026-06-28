## ADDED Requirements

### Requirement: Assemble the system prompt from embedded docs and core memory

The system message SHALL be built by embedding, at build time, `protocol.md`, then `rules.md`, then `memory.md`, then `policy.md`, joined by a `---` separator, followed by a `# Core Memory` section containing the agent's core memory (ME, USER, TODO). The four behavioural docs SHALL be compiled into the binary so the running agent never depends on the prompt files being present on disk.

#### Scenario: full prompt ordering

- **WHEN** the system prompt is assembled with `FLEETY_SYSTEM_PROMPT` unset
- **THEN** it contains protocol, rules, memory, and policy in that order, then a `# Core Memory` section with ME/USER/TODO

### Requirement: Preserve the system prompt across compaction

The assembled system prompt SHALL be placed at message index 0 and SHALL be preserved by context compaction, so it survives a context summary WITHOUT being re-sent as a separate per-turn reminder.

#### Scenario: prompt survives a summary

- **WHEN** the context is compacted mid-conversation
- **THEN** the index-0 system prompt is retained and is not duplicated as an extra reminder message

### Requirement: Minimal mode drops the static docs

When `FLEETY_SYSTEM_PROMPT=minimal` is set, the system prompt SHALL contain only the core memory (ME/USER/TODO) and SHALL omit the four embedded behavioural docs, for token-lean or debugging runs.

#### Scenario: minimal keeps only core memory

- **WHEN** `FLEETY_SYSTEM_PROMPT=minimal`
- **THEN** the system prompt is the core memory alone, without protocol/rules/memory/policy
