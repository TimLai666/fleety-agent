# inline-terminal-viewport Specification

## Purpose

TBD - created by archiving change 'inline-chat-viewport'. Update Purpose after archive.

## Requirements

### Requirement: Chat draws into an inline viewport and leaves the screen intact

Chat SHALL render into a viewport at the bottom of the terminal rather than an alternate screen. Starting Chat SHALL NOT clear or replace what the terminal was already showing, and leaving Chat SHALL leave the conversation on screen.

#### Scenario: prior terminal output survives a Chat session

- **GIVEN** the terminal shows the output of a previous command
- **WHEN** the user starts Chat, exchanges a message, and leaves
- **THEN** the previous output SHALL still be above the conversation, and the conversation SHALL still be on screen

#### Scenario: other panels are unaffected

- **WHEN** the user opens Settings from Chat
- **THEN** Settings SHALL keep using a full-screen alternate-screen terminal of its own


<!-- @trace
source: inline-chat-viewport
updated: 2026-07-27
code:
  - docs/tools.md
  - Cargo.toml
  - .opencode/commands/spectra-archive.md
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-inline/src/common.rs
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-textarea/README.md
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-markdown/src/source_map.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-markdown/Cargo.toml
  - AGENTS.md
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/src/colors.rs
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-markdown/LICENSE
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-textarea/src/lib.rs
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-cli/src/workspace.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-textarea/LICENSE
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-inline/README.md
  - crates/fleety-markdown/src/latex/math_box.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/parse.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
-->

---
### Requirement: Completed conversation content becomes terminal history

A message that is complete SHALL be written into the terminal's own scrollback as styled text, and SHALL NOT be redrawn by Fleety afterwards. Once written, its scrolling and its text selection SHALL be the terminal's responsibility.

#### Scenario: a finished exchange leaves the viewport

- **GIVEN** the user sends a message and the assistant reply completes
- **WHEN** the next frame is drawn
- **THEN** both the message and the reply SHALL be in the terminal's scrollback, and the viewport SHALL contain only the composer and the status line

#### Scenario: history outlives the process

- **GIVEN** a conversation with several exchanges
- **WHEN** the user quits Chat
- **THEN** the exchanges SHALL remain visible in the terminal


<!-- @trace
source: inline-chat-viewport
updated: 2026-07-27
code:
  - docs/tools.md
  - Cargo.toml
  - .opencode/commands/spectra-archive.md
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-inline/src/common.rs
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-textarea/README.md
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-markdown/src/source_map.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-markdown/Cargo.toml
  - AGENTS.md
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/src/colors.rs
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-markdown/LICENSE
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-textarea/src/lib.rs
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-cli/src/workspace.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-textarea/LICENSE
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-inline/README.md
  - crates/fleety-markdown/src/latex/math_box.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/parse.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
-->

---
### Requirement: A streaming reply is visible before it completes

While an assistant reply is streaming, the portion whose markdown blocks have closed SHALL be written to scrollback as it settles, and the portion still being written SHALL be drawn in the viewport and updated as new content arrives. Content SHALL NOT be written to scrollback while the markdown block it belongs to is still open.

#### Scenario: settled paragraphs move to history mid-reply

- **GIVEN** an assistant reply is streaming and its first paragraph has ended
- **WHEN** further content arrives
- **THEN** the first paragraph SHALL be in scrollback and SHALL NOT be redrawn, and the newer content SHALL be in the viewport

#### Scenario: an unclosed code fence stays in the viewport

- **GIVEN** a streaming reply has opened a fenced code block that has not closed
- **WHEN** a frame is drawn
- **THEN** the unclosed block SHALL remain in the viewport and SHALL NOT be written to scrollback

#### Scenario: the reply completes

- **WHEN** the assistant reply finishes
- **THEN** any remaining content SHALL be written to scrollback and the viewport SHALL return to the composer and the status line


<!-- @trace
source: inline-chat-viewport
updated: 2026-07-27
code:
  - docs/tools.md
  - Cargo.toml
  - .opencode/commands/spectra-archive.md
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-inline/src/common.rs
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-textarea/README.md
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-markdown/src/source_map.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-markdown/Cargo.toml
  - AGENTS.md
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/src/colors.rs
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-markdown/LICENSE
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-textarea/src/lib.rs
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-cli/src/workspace.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-textarea/LICENSE
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-inline/README.md
  - crates/fleety-markdown/src/latex/math_box.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/parse.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
-->

---
### Requirement: The viewport is bounded

The viewport SHALL be sized to the content still being written plus the composer and the status line, and SHALL NOT exceed half the terminal height. When the content exceeds that bound, the viewport SHALL scroll within itself and SHALL show the newest content.

#### Scenario: a long unsettled block does not take over the screen

- **GIVEN** a streaming reply contains an open code block longer than half the terminal height
- **WHEN** a frame is drawn
- **THEN** the viewport SHALL occupy at most half the terminal height and SHALL show the end of the block


<!-- @trace
source: inline-chat-viewport
updated: 2026-07-27
code:
  - docs/tools.md
  - Cargo.toml
  - .opencode/commands/spectra-archive.md
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-inline/src/common.rs
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-textarea/README.md
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-markdown/src/source_map.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-markdown/Cargo.toml
  - AGENTS.md
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/src/colors.rs
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-markdown/LICENSE
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-textarea/src/lib.rs
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-cli/src/workspace.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-textarea/LICENSE
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-inline/README.md
  - crates/fleety-markdown/src/latex/math_box.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/parse.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
-->

---
### Requirement: Resizing preserves the history

When the terminal size changes, Fleety SHALL re-emit the conversation history it has written rather than relying on the terminal's own reflow.

#### Scenario: narrowing the terminal does not corrupt earlier output

- **GIVEN** a conversation with several exchanges in scrollback
- **WHEN** the terminal is made narrower and a frame is drawn
- **THEN** the history SHALL be re-emitted at the new width with the same content and no leftover fragments


<!-- @trace
source: inline-chat-viewport
updated: 2026-07-27
code:
  - docs/tools.md
  - Cargo.toml
  - .opencode/commands/spectra-archive.md
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-inline/src/common.rs
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-textarea/README.md
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-markdown/src/source_map.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-markdown/Cargo.toml
  - AGENTS.md
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/src/colors.rs
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-markdown/LICENSE
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-textarea/src/lib.rs
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-cli/src/workspace.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-textarea/LICENSE
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-inline/README.md
  - crates/fleety-markdown/src/latex/math_box.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/parse.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
-->

---
### Requirement: Fleety does not take the mouse

Fleety SHALL NOT enable terminal mouse reporting, and its input pipeline SHALL carry keyboard events only. Scrolling the conversation and selecting its text SHALL be performed with the terminal's own mouse handling.

#### Scenario: dragging selects with the terminal

- **GIVEN** Chat is running
- **WHEN** the user drags across the conversation without holding any modifier
- **THEN** the terminal SHALL perform its own text selection and Fleety SHALL receive no event

#### Scenario: the wheel scrolls the terminal's history

- **WHEN** the user scrolls the wheel
- **THEN** the terminal SHALL scroll its own scrollback and Fleety SHALL NOT change what it draws

<!-- @trace
source: inline-chat-viewport
updated: 2026-07-27
code:
  - docs/tools.md
  - Cargo.toml
  - .opencode/commands/spectra-archive.md
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-cli/src/input.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-inline/src/common.rs
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-textarea/README.md
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-markdown/src/source_map.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-markdown/Cargo.toml
  - AGENTS.md
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/src/colors.rs
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-markdown/LICENSE
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-textarea/src/lib.rs
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-cli/src/workspace.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-textarea/LICENSE
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-inline/README.md
  - crates/fleety-markdown/src/latex/math_box.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/parse.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
-->