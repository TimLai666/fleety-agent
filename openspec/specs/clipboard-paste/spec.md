# clipboard-paste Specification

## Purpose

TBD - created by archiving change 'cli-clipboard-acp-polish'. Update Purpose after archive.

## Requirements

### Requirement: Clipboard file attachments are size-bounded

When the TUI turns a pasted file path into a message attachment, it SHALL enforce a maximum attachment byte size (a compile-time constant with a sane default) and SHALL NOT silently read and base64-encode a file larger than that limit. When a file exceeds the limit, the paste SHALL fall back to inserting the file path as text so the user sees why it was not attached, instead of embedding a large or binary blob into the message.

#### Scenario: an oversized file is not attached

- **WHEN** the clipboard holds a path to a file larger than the maximum attachment size
- **THEN** the TUI does not base64-encode the file and instead inserts the path as text

#### Scenario: a within-limit file attaches

- **WHEN** the clipboard holds a path to a file within the size limit
- **THEN** the TUI attaches the file bytes as before


<!-- @trace
source: cli-clipboard-acp-polish
updated: 2026-07-10
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/config.rs
  - docs/env.md
  - crates/fleety-server/src/restart_watch.rs
  - Dockerfile
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/privacy.rs
  - scripts/install.sh
  - crates/fleety-cli/src/main.rs
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/clipboard.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Clipboard file attachments carry an identifiable type

A clipboard file attachment SHALL carry a MIME type and the original filename (with extension) so the server can determine how to handle it. Source-code files (for example `.rs`, `.py`, `.js`, `.ts`, `.go`) SHALL be attached with a type that identifies them as text and preserves the language signal — a language-specific `text/*` type where one is defined, and always the filename with its extension — instead of a bare `text/plain` that discards the language. Files with no recognized type SHALL be labeled `application/octet-stream` with their filename preserved.

#### Scenario: a source file keeps its language signal

- **WHEN** the user pastes a path to a `.rs` file within the size limit
- **THEN** the attachment carries the `.rs` filename and a text MIME type that lets the server identify it as source, not an opaque blob

#### Scenario: an unknown type is octet-stream

- **WHEN** the user pastes a path to a file with no recognized extension
- **THEN** the attachment is labeled `application/octet-stream` with its filename preserved

<!-- @trace
source: cli-clipboard-acp-polish
updated: 2026-07-10
code:
  - crates/fleety-server/src/conn.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-cli/src/config.rs
  - docs/env.md
  - crates/fleety-server/src/restart_watch.rs
  - Dockerfile
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-server/src/privacy.rs
  - scripts/install.sh
  - crates/fleety-cli/src/main.rs
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/clipboard.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->