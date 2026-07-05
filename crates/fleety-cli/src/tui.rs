//! Interactive multi-pane TUI (ratatui): a conversation pane, an input box, and
//! a status line. The `App` state, key handling, and rendering are unit-tested
//! (ratatui `TestBackend` renders to an in-memory buffer); the live
//! terminal/event/WebSocket loop in `main.rs` is the glue around them.
//!
//! Clipboard integration mirrors how Claude Code handles paste: Ctrl+V asks the
//! outer loop to peek at the OS clipboard. If the clipboard holds an image, it
//! becomes a PNG attachment. If it holds the path of an existing file, that
//! file is attached directly. Otherwise the clipboard text is pasted into the
//! input.

use std::collections::VecDeque;

use fleety_protocol::WireAttachment;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::input::LineEditor;

/// Lines jumped per PageUp/PageDown press.
const SCROLL_STEP: u16 = 5;

/// TUI state.
pub struct App {
    pub messages: Vec<(String, String)>,
    pub input: LineEditor,
    pub status: String,
    pub should_quit: bool,
    /// Attachments staged for the next `Send`. Cleared automatically when the
    /// user submits the line.
    pub pending_attachments: Vec<WireAttachment>,
    /// Whether the last message is an assistant reply still streaming in.
    streaming: bool,
    /// Wrapped lines scrolled up from the bottom of the conversation
    /// (0 = follow the newest output). Clamped to the content at render time.
    pub scroll_back: u16,
    /// Approvals awaiting a y/n decision, oldest first: (approval_id, summary).
    /// While non-empty the input is modal — only y / n / Esc act.
    pub pending_approvals: VecDeque<(String, String)>,
}

impl App {
    pub fn new(status: impl Into<String>) -> Self {
        Self {
            messages: Vec::new(),
            input: LineEditor::default(),
            status: status.into(),
            should_quit: false,
            pending_attachments: Vec::new(),
            streaming: false,
            scroll_back: 0,
            pending_approvals: VecDeque::new(),
        }
    }

    pub fn push(&mut self, role: impl Into<String>, text: impl Into<String>) {
        self.messages.push((role.into(), text.into()));
        self.streaming = false;
    }

    /// Append a streamed assistant chunk to the in-progress reply.
    pub fn push_delta(&mut self, chunk: &str) {
        if self.streaming {
            if let Some(last) = self.messages.last_mut() {
                last.1.push_str(chunk);
                return;
            }
        }
        self.messages
            .push(("fleety".to_string(), chunk.to_string()));
        self.streaming = true;
    }

    /// Finalize the assistant reply with the authoritative full text.
    pub fn finish_assistant(&mut self, text: String) {
        if self.streaming {
            if let Some(last) = self.messages.last_mut() {
                last.1 = text;
            }
            self.streaming = false;
        } else {
            self.push("fleety", text);
        }
    }

    /// Stage one attachment for the next send. The outer loop calls this after
    /// resolving a clipboard image or a file path.
    pub fn attach(&mut self, attachment: WireAttachment) {
        let label = attachment
            .name
            .clone()
            .unwrap_or_else(|| attachment.mime.clone());
        self.pending_attachments.push(attachment);
        self.status = format!(
            "attached {label} ({} pending)",
            self.pending_attachments.len()
        );
    }

    /// Drop any staged attachments without sending.
    pub fn clear_attachments(&mut self) {
        if !self.pending_attachments.is_empty() {
            self.status = format!("cleared {} attachment(s)", self.pending_attachments.len());
            self.pending_attachments.clear();
        }
    }

    /// Title shown on the input box — includes a paperclip count when there
    /// are staged attachments so the user can see what'll be sent.
    pub fn input_title(&self) -> String {
        if let Some((_, desc)) = self.pending_approvals.front() {
            return format!("Approval required — y=approve · n=deny — {desc}");
        }
        if self.pending_attachments.is_empty() {
            "Message (Enter=send, Ctrl+V=paste, PgUp/PgDn=scroll, Esc=quit)".to_string()
        } else {
            format!(
                "Message [{} attached] (Enter=send, Ctrl+X=drop attachments, Esc=quit)",
                self.pending_attachments.len()
            )
        }
    }

    /// Queue an approval request from the server. Shown in the conversation
    /// pane and answered modally with y/n (see `on_key`).
    pub fn request_approval(&mut self, approval_id: String, tool: &str, risk: &str, summary: &str) {
        let desc = format!("{tool} ({risk}): {summary}");
        self.push("approval", desc.clone());
        self.pending_approvals.push_back((approval_id, desc));
        self.status = "approval required — y approve · n deny".to_string();
    }
}

/// What a keypress asks the outer loop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Submit the current input + any staged attachments.
    Send {
        text: String,
        attachments: Vec<WireAttachment>,
    },
    Quit,
    /// Ctrl+V: outer loop should consult the OS clipboard (image, file path,
    /// or plain text) and update the app accordingly.
    PasteFromClipboard,
    /// y on a pending approval: send `ClientMsg::Approve` for this id.
    Approve(String),
    /// n/Esc on a pending approval: send `ClientMsg::Deny` for this id.
    Deny(String),
}

/// Apply a keypress to the app, returning the action for the loop to perform.
pub fn on_key(app: &mut App, key: KeyEvent) -> Action {
    // Ctrl+C always quits, even while an approval is pending.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            if c.to_ascii_lowercase() == 'c' {
                app.should_quit = true;
                return Action::Quit;
            }
        }
    }
    // Scrollback works in every mode; clamping happens at render time.
    // (Home/End belong to the input cursor below, not to scrolling.)
    match key.code {
        KeyCode::PageUp => {
            app.scroll_back = app.scroll_back.saturating_add(SCROLL_STEP);
            return Action::None;
        }
        KeyCode::PageDown => {
            app.scroll_back = app.scroll_back.saturating_sub(SCROLL_STEP);
            return Action::None;
        }
        _ => {}
    }
    // A pending approval is modal: only y / n / Esc act until it's answered
    // (the server's gate ignores other client messages while waiting anyway).
    if !app.pending_approvals.is_empty() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                if let Some((id, desc)) = app.pending_approvals.pop_front() {
                    app.status = format!("approved: {desc}");
                    return Action::Approve(id);
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                if let Some((id, desc)) = app.pending_approvals.pop_front() {
                    app.status = format!("denied: {desc}");
                    return Action::Deny(id);
                }
            }
            _ => {}
        }
        return Action::None;
    }
    // Ctrl-prefixed shortcuts come before plain Char fallthrough so it doesn't
    // swallow them (e.g. Ctrl+V should NOT type 'v').
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char(c) => match c.to_ascii_lowercase() {
                'v' => return Action::PasteFromClipboard,
                'x' => {
                    app.clear_attachments();
                    return Action::None;
                }
                _ => {}
            },
            KeyCode::Left => {
                app.input.word_left();
                return Action::None;
            }
            KeyCode::Right => {
                app.input.word_right();
                return Action::None;
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::Esc => {
            app.should_quit = true;
            Action::Quit
        }
        KeyCode::Enter => {
            let text = app.input.take();
            let attachments = std::mem::take(&mut app.pending_attachments);
            if text.trim().is_empty() && attachments.is_empty() {
                Action::None
            } else {
                let display = if attachments.is_empty() {
                    text.clone()
                } else {
                    format!("{text} [+{} attachment(s)]", attachments.len())
                };
                app.push("you", display);
                // Sending snaps the conversation back to the newest output.
                app.scroll_back = 0;
                Action::Send { text, attachments }
            }
        }
        KeyCode::Backspace => {
            app.input.backspace();
            Action::None
        }
        KeyCode::Delete => {
            app.input.delete();
            Action::None
        }
        KeyCode::Left => {
            app.input.left();
            Action::None
        }
        KeyCode::Right => {
            app.input.right();
            Action::None
        }
        KeyCode::Home => {
            app.input.home();
            Action::None
        }
        KeyCode::End => {
            app.input.end();
            Action::None
        }
        KeyCode::Char(c) => {
            app.input.insert(c);
            Action::None
        }
        _ => Action::None,
    }
}

/// Flatten messages into display lines. Embedded newlines must become real
/// `Line`s — a ratatui `Line` is single-line and drops `\n` (the segments
/// would be glued together with no break).
fn message_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (role, text) in &app.messages {
        for (i, part) in text.split('\n').enumerate() {
            if i == 0 {
                lines.push(Line::from(format!("{role}: {part}")));
            } else {
                lines.push(Line::from(part.to_string()));
            }
        }
    }
    lines
}

/// Draw the three panes.
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // Anchor the conversation to the bottom (newest output), minus any manual
    // scrollback; the wrapped height comes from the block-less paragraph so
    // the count and the render agree on the text width.
    let para = Paragraph::new(message_lines(app)).wrap(Wrap { trim: false });
    let inner_w = chunks[0].width.saturating_sub(2);
    let inner_h = chunks[0].height.saturating_sub(2);
    let total = u16::try_from(para.line_count(inner_w)).unwrap_or(u16::MAX);
    let max_off = total.saturating_sub(inner_h);
    let offset = max_off.saturating_sub(app.scroll_back.min(max_off));
    frame.render_widget(
        para.block(Block::bordered().title("Fleety"))
            .scroll((offset, 0)),
        chunks[0],
    );

    // Show the slice of the input that keeps the cursor visible (the editor
    // scrolls horizontally on overflow) and park the terminal cursor on it.
    let max_w = chunks[1].width.saturating_sub(2) as usize;
    let (view, cursor_x) = app.input.display_window(max_w);
    frame.render_widget(
        Paragraph::new(view).block(Block::bordered().title(app.input_title())),
        chunks[1],
    );
    frame.set_cursor_position((chunks[1].x + 1 + cursor_x, chunks[1].y + 1));

    frame.render_widget(Paragraph::new(app.status.as_str()), chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn typing_enter_and_quit() {
        let mut app = App::new("ready");
        assert_eq!(on_key(&mut app, key(KeyCode::Char('h'))), Action::None);
        on_key(&mut app, key(KeyCode::Char('i')));
        assert_eq!(app.input.text(), "hi");
        on_key(&mut app, key(KeyCode::Backspace));
        assert_eq!(app.input.text(), "h");
        assert_eq!(
            on_key(&mut app, key(KeyCode::Enter)),
            Action::Send {
                text: "h".to_string(),
                attachments: Vec::new(),
            }
        );
        assert_eq!(app.input.text(), ""); // cleared on send
        assert_eq!(app.messages.last().map(|(r, _)| r.as_str()), Some("you"));
        // Empty Enter with no attachments does nothing.
        assert_eq!(on_key(&mut app, key(KeyCode::Enter)), Action::None);
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_v_routes_to_paste_action() {
        let mut app = App::new("ready");
        assert_eq!(on_key(&mut app, ctrl('v')), Action::PasteFromClipboard);
        // The 'v' character must not also leak into the input buffer.
        assert!(app.input.is_empty());
    }

    #[test]
    fn enter_sends_attachments_then_clears_them() {
        let mut app = App::new("ready");
        app.attach(WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("AAAA".into()),
            url: None,
            name: Some("paste.png".into()),
        });
        on_key(&mut app, key(KeyCode::Char('s')));
        on_key(&mut app, key(KeyCode::Char('e')));
        on_key(&mut app, key(KeyCode::Char('e')));
        let Action::Send { text, attachments } = on_key(&mut app, key(KeyCode::Enter)) else {
            panic!("expected Send");
        };
        assert_eq!(text, "see");
        assert_eq!(attachments.len(), 1);
        assert!(app.pending_attachments.is_empty(), "cleared after send");
    }

    #[test]
    fn enter_with_only_attachments_still_sends() {
        let mut app = App::new("ready");
        app.attach(WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("AAAA".into()),
            url: None,
            name: Some("paste.png".into()),
        });
        let Action::Send { text, attachments } = on_key(&mut app, key(KeyCode::Enter)) else {
            panic!("expected Send with attachments only");
        };
        assert!(text.is_empty());
        assert_eq!(attachments.len(), 1);
    }

    #[test]
    fn ctrl_x_clears_pending_attachments() {
        let mut app = App::new("ready");
        app.attach(WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("AAAA".into()),
            url: None,
            name: None,
        });
        assert_eq!(app.pending_attachments.len(), 1);
        assert_eq!(on_key(&mut app, ctrl('x')), Action::None);
        assert!(app.pending_attachments.is_empty());
    }

    #[test]
    fn input_title_reflects_attachment_count() {
        let mut app = App::new("ready");
        assert!(app.input_title().contains("Ctrl+V=paste"));
        app.attach(WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("AAAA".into()),
            url: None,
            name: None,
        });
        assert!(app.input_title().contains("1 attached"));
    }

    #[test]
    fn streaming_deltas_then_finalize() {
        let mut app = App::new("ready");
        app.push("you", "hi");
        app.push_delta("Hel");
        app.push_delta("lo");
        assert_eq!(app.messages.last().map(|(_, t)| t.as_str()), Some("Hello"));
        // The final authoritative text replaces the streamed accumulation.
        app.finish_assistant("Hello!".to_string());
        assert_eq!(app.messages.last().map(|(_, t)| t.as_str()), Some("Hello!"));
        // A fresh reply with no prior deltas just appends.
        app.finish_assistant("again".to_string());
        assert_eq!(
            app.messages.last(),
            Some(&("fleety".to_string(), "again".to_string()))
        );
    }

    #[test]
    fn renders_all_panes() {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).expect("term");
        let mut app = App::new("connected");
        app.push("fleety", "hello there");
        app.input.set_text("typing".into());
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("hello there"), "messages pane");
        assert!(content.contains("typing"), "input pane");
        assert!(content.contains("Fleety"), "title");
        assert!(content.contains("connected"), "status line");
    }

    /// One terminal row per rendered line, for asserting what's visible where.
    fn visible_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buf = terminal.backend().buffer();
        let w = buf.area.width as usize;
        let syms: Vec<&str> = buf.content().iter().map(|c| c.symbol()).collect();
        syms.chunks(w).map(|r| r.concat()).collect()
    }

    #[test]
    fn multiline_message_renders_as_separate_lines() {
        let mut app = App::new("ready");
        app.push("fleety", "first line\nsecond line");
        // The flattener splits on \n (a single ratatui Line would drop it).
        assert_eq!(message_lines(&app).len(), 2);

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let rows = visible_rows(&terminal);
        let first = rows.iter().position(|r| r.contains("first line"));
        let second = rows.iter().position(|r| r.contains("second line"));
        assert!(first.is_some() && second.is_some(), "both lines visible");
        assert_ne!(first, second, "on different rows");
        assert!(
            !rows.iter().any(|r| r.contains("first linesecond")),
            "not glued together"
        );
    }

    #[test]
    fn conversation_follows_the_bottom_when_it_overflows() {
        // 9 rows: messages pane gets 5 (3 inner), input 3, status 1.
        let mut terminal = Terminal::new(TestBackend::new(30, 9)).expect("term");
        let mut app = App::new("ready");
        for i in 1..=8 {
            app.push("x", format!("m{i}"));
        }
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = visible_rows(&terminal).concat();
        assert!(content.contains("m8"), "newest message visible");
        assert!(!content.contains("m1 "), "oldest scrolled out");
    }

    #[test]
    fn page_up_scrolls_back_and_sending_returns_to_bottom() {
        let mut terminal = Terminal::new(TestBackend::new(30, 9)).expect("term");
        let mut app = App::new("ready");
        for i in 1..=8 {
            app.push("x", format!("m{i}"));
        }
        assert_eq!(on_key(&mut app, key(KeyCode::PageUp)), Action::None);
        assert_eq!(app.scroll_back, SCROLL_STEP);
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = visible_rows(&terminal).concat();
        assert!(content.contains("m1"), "scrolled back to the top");
        assert!(!content.contains("m8"), "bottom out of view");

        // PageDown steps back toward the bottom.
        assert_eq!(on_key(&mut app, key(KeyCode::PageDown)), Action::None);
        assert_eq!(app.scroll_back, 0);

        // End no longer touches the scrollback — it's an input-cursor key now.
        on_key(&mut app, key(KeyCode::PageUp));
        assert_eq!(on_key(&mut app, key(KeyCode::End)), Action::None);
        assert_eq!(app.scroll_back, SCROLL_STEP, "End left scrollback alone");

        // Sending a message snaps back to the newest output.
        on_key(&mut app, key(KeyCode::Char('z')));
        assert!(matches!(
            on_key(&mut app, key(KeyCode::Enter)),
            Action::Send { .. }
        ));
        assert_eq!(app.scroll_back, 0);
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = visible_rows(&terminal).concat();
        assert!(content.contains("you: z"), "sent line at the bottom");
    }

    #[test]
    fn cursor_keys_edit_in_the_middle() {
        let mut app = App::new("ready");
        for c in "ab漢".chars() {
            on_key(&mut app, key(KeyCode::Char(c)));
        }
        // Left + insert lands before the CJK char, on a char boundary.
        on_key(&mut app, key(KeyCode::Left));
        on_key(&mut app, key(KeyCode::Char('X')));
        assert_eq!(app.input.text(), "abX漢");
        // Home/Delete eat the first char; End/Backspace eat the last.
        on_key(&mut app, key(KeyCode::Home));
        on_key(&mut app, key(KeyCode::Delete));
        assert_eq!(app.input.text(), "bX漢");
        on_key(&mut app, key(KeyCode::End));
        on_key(&mut app, key(KeyCode::Backspace));
        assert_eq!(app.input.text(), "bX");
        // Ctrl+Left jumps to the word start; typing prepends.
        on_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
        );
        on_key(&mut app, key(KeyCode::Char('!')));
        assert_eq!(app.input.text(), "!bX");
    }

    #[test]
    fn approval_is_modal_and_answered_with_y_n() {
        let mut app = App::new("ready");
        app.request_approval("id1".into(), "write_file", "mutate", "write foo.txt");
        assert!(app.input_title().contains("Approval required"));
        // Typing and Enter are swallowed while the approval is pending.
        assert_eq!(on_key(&mut app, key(KeyCode::Char('h'))), Action::None);
        assert!(app.input.is_empty());
        assert_eq!(on_key(&mut app, key(KeyCode::Enter)), Action::None);
        // y approves.
        assert_eq!(
            on_key(&mut app, key(KeyCode::Char('y'))),
            Action::Approve("id1".into())
        );
        assert!(app.pending_approvals.is_empty());
        // Esc on a pending approval denies — it does not quit.
        app.request_approval("id2".into(), "run_command", "critical", "rm x");
        assert_eq!(
            on_key(&mut app, key(KeyCode::Esc)),
            Action::Deny("id2".into())
        );
        assert!(!app.should_quit);
        // With no approval pending, Esc quits as before.
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
    }
}
