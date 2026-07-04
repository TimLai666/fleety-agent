## ADDED Requirements

### Requirement: The Codex user-global AGENTS.md is injected

The conversation's user-global instruction files SHALL include the originating device's `~/.codex/AGENTS.md`, alongside `~/.claude/CLAUDE.md` and `~/.agents/AGENTS.md`, when it is present. It joins the user-global layer as a soft overlay like the other user-global files. Best-effort: an absent `~/.codex/AGENTS.md` is simply skipped.

#### Scenario: Codex AGENTS.md joins the user-global instruction files

- **WHEN** a same-host conversation binds and `~/.codex/AGENTS.md` exists on the originating device
- **THEN** its content is injected as part of the conversation's user-global instruction files
