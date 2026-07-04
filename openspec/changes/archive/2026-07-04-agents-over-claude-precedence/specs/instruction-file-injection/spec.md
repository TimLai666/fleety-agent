## ADDED Requirements

### Requirement: Same-layer instruction precedence favors AGENTS.md over CLAUDE.md

At any single directory layer, the injected `AGENTS.md` SHALL be ordered at a more-specific position than the same layer's `CLAUDE.md`, so that when the two conflict the generic `AGENTS.md` instructions take precedence over the Claude-specific `CLAUDE.md` ones. Because injection is a soft, ordered overlay where deeper / later text is treated as more specific, this means a layer's `AGENTS.md` follows (comes after) that layer's `CLAUDE.md`. The shallow-to-deep layer ordering and the scope are unchanged.

#### Scenario: AGENTS.md ranks above CLAUDE.md at the same layer

- **WHEN** a directory layer contains both `AGENTS.md` and `CLAUDE.md`
- **THEN** the layer's `AGENTS.md` is injected at a more-specific (higher-priority) position than its `CLAUDE.md`
