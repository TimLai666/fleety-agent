# interactive-chat-tui Specification

## Purpose

TBD - created by archiving change 'tui-depth'. Update Purpose after archive.

## Requirements

### Requirement: Rich Assistant Rendering

The chat TUI SHALL render assistant replies as structured rich text rather than a single flattened plain-text block. It SHALL recognize a common markdown subset (ATX headings, ordered and unordered lists, blockquotes, inline `code`, and `**bold**` emphasis) and SHALL render fenced code blocks in a monospaced style that is visually distinguished from surrounding prose. Content that does not match any recognized markdown construct SHALL be rendered as plain text without error.

#### Scenario: Fenced code block is set apart from prose

- **WHEN** an assistant reply contains a triple-backtick fenced code block surrounded by ordinary sentences
- **THEN** the code block SHALL render as monospaced lines visually separated from the prose, and the fenced-block content SHALL NOT have its inline markdown interpreted

#### Scenario: Inline emphasis and code are styled

- **WHEN** an assistant reply contains `**bold**` and inline `` `code` `` spans on a normal line
- **THEN** the bold span SHALL render with an emphasized style and the inline-code span with a code style, while the delimiter characters are not shown as literal text

#### Scenario: Unrecognized syntax degrades safely

- **WHEN** an assistant reply contains an unterminated fenced block or malformed markdown
- **THEN** the renderer SHALL fall back to plain-text lines without panicking or dropping content


<!-- @trace
source: tui-depth
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - scripts/install.sh
  - README.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - docs/env.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/markdown.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Waiting Indicator With Animated Spinner

While a turn is in flight, the chat TUI SHALL display an animated spinner that advances on a fixed-interval tick independently of incoming server frames, so the user sees continuous progress even when no new output arrives. When the turn is idle the spinner SHALL NOT animate or force redraws.

#### Scenario: Spinner advances without new server frames

- **WHEN** a turn is in flight and no server frame arrives between two spinner ticks
- **THEN** the spinner frame index SHALL advance on the tick and the waiting indicator SHALL redraw

#### Scenario: Spinner is quiet when idle

- **WHEN** no turn is in flight
- **THEN** the spinner SHALL remain static and SHALL NOT trigger periodic redraws


<!-- @trace
source: tui-depth
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - scripts/install.sh
  - README.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - docs/env.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/markdown.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Multi-Line Message Composition

The chat TUI input SHALL support composing a message across multiple lines. A dedicated newline key (Alt+Enter, with Ctrl+J accepted as a compatibility binding) SHALL insert a line break into the input without submitting, while a bare Enter SHALL submit the entire multi-line buffer. UTF-8 and CJK char-boundary safety of the editor SHALL be preserved.

#### Scenario: Newline key inserts a break without sending

- **WHEN** the user presses Alt+Enter (or Ctrl+J) while composing
- **THEN** a line break SHALL be inserted at the cursor and no message SHALL be submitted

#### Scenario: Bare Enter submits the whole buffer

- **WHEN** the input contains two lines separated by an inserted break and the user presses Enter
- **THEN** the submitted message text SHALL contain both lines including the embedded newline


<!-- @trace
source: tui-depth
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - scripts/install.sh
  - README.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - docs/env.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/markdown.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Typed File Attachment Command

The chat TUI SHALL let the user attach a local file by typing a command rather than only via clipboard paste. An input consisting of `/attach <path>` SHALL be interpreted as an attach request: when `<path>` resolves to an existing file it SHALL be staged as a pending attachment (reusing the clipboard file-detection and MIME guessing) and the input SHALL be cleared; when it does not resolve the input SHALL be preserved and an error SHALL be reported in the status line. The command SHALL NOT be sent as an ordinary message.

#### Scenario: Attach an existing file by command

- **WHEN** the user submits `/attach ./notes.txt` and that file exists
- **THEN** the file SHALL be added to the pending attachments, the input SHALL be cleared, and no chat message SHALL be sent for that submission

#### Scenario: Attach a missing path reports an error

- **WHEN** the user submits `/attach /no/such/file` that does not exist
- **THEN** no attachment SHALL be staged, the input SHALL be preserved, and the status line SHALL report that the path could not be attached


<!-- @trace
source: tui-depth
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - scripts/install.sh
  - README.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - docs/env.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/markdown.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Automatic Reconnection With Backoff

When the server connection drops, the chat TUI SHALL attempt to reconnect automatically using capped exponential backoff instead of exiting immediately. On a successful reconnect it SHALL restore the active conversation by resuming from the last observed event sequence (`Resume`/`Replay`), de-duplicating replayed events already shown. The user SHALL be able to abort the waiting with Ctrl+C, and the TUI SHALL exit cleanly only after the backoff attempts are exhausted.

#### Scenario: Reconnect restores the conversation

- **WHEN** the connection drops mid-session and a reconnect attempt succeeds
- **THEN** the TUI SHALL send a `Resume` for the active conversation from the last seen sequence and SHALL apply the replayed events without duplicating already-displayed messages

#### Scenario: Exhausted backoff exits cleanly

- **WHEN** every backoff reconnect attempt fails
- **THEN** the TUI SHALL stop retrying, report the disconnection in the status line, and exit without crashing


<!-- @trace
source: tui-depth
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - scripts/install.sh
  - README.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - docs/env.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/markdown.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Unsent-Input Quit Confirmation

When the user triggers quit (Esc while idle) and the input buffer or pending attachments are non-empty, the chat TUI SHALL require a confirmation before discarding that content: the first Esc SHALL enter a confirm state that warns about unsent content, and only a second consecutive Esc SHALL quit. Any editing keypress SHALL cancel the confirm state. When there is no unsent content, Esc SHALL quit directly, and Ctrl+C SHALL always quit immediately regardless of unsent content.

#### Scenario: Esc with unsent input asks for confirmation

- **WHEN** the input buffer is non-empty and the user presses Esc while idle
- **THEN** the TUI SHALL enter a quit-confirm state and SHALL NOT quit until a second Esc is pressed

#### Scenario: Editing cancels the confirm state

- **WHEN** the TUI is in the quit-confirm state and the user presses an editing key
- **THEN** the confirm state SHALL be cleared and the next Esc SHALL again require confirmation

#### Scenario: Ctrl+C bypasses confirmation

- **WHEN** the input buffer is non-empty and the user presses Ctrl+C
- **THEN** the TUI SHALL quit immediately without a confirmation step

<!-- @trace
source: tui-depth
updated: 2026-07-10
code:
  - Dockerfile
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-server/src/storage.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-protocol/src/lib.rs
  - scripts/install.sh
  - README.md
  - crates/fleety-server/src/identity.rs
  - crates/fleety-server/src/scheduler.rs
  - docs/env.md
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/markdown.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->