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
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::input::LineEditor;

/// Lines jumped per PageUp/PageDown press.
const SCROLL_STEP: u16 = 5;

/// Braille spinner frames, advanced on a fixed tick while a turn is in flight
/// (or during reconnection) so the wait shows motion even with no new frames.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    /// Whether a turn is running (message sent, final reply not yet received).
    /// Set on `Send`, cleared when the reply, an error, or a disconnect lands.
    /// Drives Esc = cancel vs. Esc = quit.
    pub turn_in_flight: bool,
    /// Animated spinner frame. Advanced only by the outer loop's fixed tick
    /// while `turn_in_flight` (or reconnecting), so idle stays static.
    pub spinner_frame: usize,
    /// Last conversation id observed (from Assistant / AssistantDelta / Replay).
    /// Used to `Resume` the right conversation after a reconnect.
    pub last_conversation_id: Option<String>,
    /// Highest event `seq` observed. Sent as `after_seq` on `Resume`, and used
    /// to de-duplicate replayed events that were already shown.
    pub last_seq: u64,
    /// Whether an idle Esc is awaiting a second, confirming Esc because there is
    /// unsent input or a pending attachment. Cleared by any editing keypress.
    pub confirm_quit: bool,
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
            turn_in_flight: false,
            spinner_frame: 0,
            last_conversation_id: None,
            last_seq: 0,
            confirm_quit: false,
        }
    }

    /// Record the conversation id and (monotonic) `seq` of an authoritative
    /// event so a later reconnect can `Resume` from exactly here.
    pub fn note_seq(&mut self, conversation_id: &str, seq: u64) {
        self.last_conversation_id = Some(conversation_id.to_string());
        if seq > self.last_seq {
            self.last_seq = seq;
        }
    }

    /// Apply a replayed past event, de-duplicating any already shown. Returns
    /// `true` when the event was newly appended, `false` when skipped as a
    /// duplicate (`seq` at or below the highest already seen).
    pub fn apply_replay(
        &mut self,
        conversation_id: &str,
        seq: u64,
        role: &str,
        content: &str,
    ) -> bool {
        self.last_conversation_id = Some(conversation_id.to_string());
        if seq <= self.last_seq {
            return false;
        }
        self.push(role, content);
        self.last_seq = seq;
        true
    }

    /// Advance the spinner one frame. Driven by the outer loop's fixed-interval
    /// tick while waiting; wraps without panicking.
    pub fn advance_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
    }

    /// The current spinner glyph.
    pub fn spinner_char(&self) -> &'static str {
        SPINNER[self.spinner_frame % SPINNER.len()]
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

    /// Finalize the assistant reply with the authoritative full text. This is
    /// the turn's terminal message, so the in-flight state clears here.
    pub fn finish_assistant(&mut self, text: String) {
        if self.streaming {
            if let Some(last) = self.messages.last_mut() {
                last.1 = text;
            }
            self.streaming = false;
        } else {
            self.push("fleety", text);
        }
        self.turn_in_flight = false;
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
        if self.turn_in_flight {
            return format!("{} Working… (Esc=cancel, Ctrl+C=quit)", self.spinner_char());
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
    /// Esc while a turn is in flight: send `ClientMsg::CancelTurn`.
    CancelTurn,
}

/// If `input` is the `/attach <path>` command, return the path argument (an
/// empty string when the command was typed with no path). `None` means it is
/// not the command, so it should be sent as an ordinary message.
fn attach_command_path(input: &str) -> Option<&str> {
    let rest = input.trim().strip_prefix("/attach")?;
    if rest.is_empty() {
        return Some("");
    }
    if rest.starts_with(char::is_whitespace) {
        return Some(rest.trim());
    }
    // e.g. "/attachment …" is a normal message, not the attach command.
    None
}

/// Apply a keypress to the app, returning the action for the loop to perform.
pub fn on_key(app: &mut App, key: KeyEvent) -> Action {
    // Ctrl+C always quits, even while an approval is pending or unsent input is
    // waiting for a quit confirmation.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            if c.eq_ignore_ascii_case(&'c') {
                app.should_quit = true;
                return Action::Quit;
            }
        }
    }
    // Any key other than Esc cancels a pending quit-confirmation, so the user
    // must press Esc twice again to discard unsent content.
    if app.confirm_quit && key.code != KeyCode::Esc {
        app.confirm_quit = false;
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
                'j' => {
                    // Ctrl+J: compatibility newline key for terminals that can't
                    // deliver Alt+Enter distinctly.
                    app.input.insert_newline();
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
            // A turn in flight: Esc cancels it (does not quit). Idle: Esc quits,
            // but unsent input / a pending attachment first needs a second Esc.
            if app.turn_in_flight {
                app.status = "cancelling — stopping at the next safe point…".to_string();
                Action::CancelTurn
            } else if !app.confirm_quit
                && (!app.input.is_empty() || !app.pending_attachments.is_empty())
            {
                app.confirm_quit = true;
                app.status =
                    "unsent content — press Esc again to discard and quit, or keep editing"
                        .to_string();
                Action::None
            } else {
                app.should_quit = true;
                Action::Quit
            }
        }
        KeyCode::Enter => {
            // Alt+Enter inserts a line break instead of submitting (Shift+Enter
            // is unreliable across terminals; Ctrl+J is the other newline key).
            if key.modifiers.contains(KeyModifiers::ALT) {
                app.input.insert_newline();
                return Action::None;
            }
            let text = app.input.take();
            // `/attach <path>` stages a local file instead of sending a message.
            if let Some(path) = attach_command_path(&text) {
                if path.is_empty() {
                    app.status = "usage: /attach <path>".to_string();
                    app.input.set_text(text);
                } else if let Some(att) = crate::clipboard::attach_path(path) {
                    app.attach(att);
                } else {
                    // `attach_path` returns `None` for a missing/unreadable path
                    // *or* a file past the size limit — cover both so an oversized
                    // file is not misreported as "no such file".
                    app.status = format!(
                        "could not attach '{path}' — no such file, or larger than the {} MiB limit",
                        crate::clipboard::MAX_ATTACHMENT_BYTES / (1024 * 1024)
                    );
                    app.input.set_text(text);
                }
                return Action::None;
            }
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
                app.turn_in_flight = true;
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
        KeyCode::Up => {
            app.input.up();
            Action::None
        }
        KeyCode::Down => {
            app.input.down();
            Action::None
        }
        KeyCode::Char(c) => {
            app.input.insert(c);
            Action::None
        }
        _ => Action::None,
    }
}

/// Flatten messages into display lines. Assistant (`fleety`) replies are given
/// rich markdown rendering; the role label is folded onto the first rendered
/// line so the flattened line count still tracks the source. User and system
/// messages stay plain, split per embedded newline — a ratatui `Line` is
/// single-line and drops `\n`, so the segments must become real `Line`s or they
/// would be glued together with no break.
fn message_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (role, text) in &app.messages {
        if role == "fleety" {
            let mut rendered = crate::markdown::render(text);
            match rendered.first_mut() {
                Some(first) => first.spans.insert(0, Span::raw(format!("{role}: "))),
                None => rendered.push(Line::from(format!("{role}: "))),
            }
            lines.extend(rendered);
        } else {
            for (i, part) in text.split('\n').enumerate() {
                if i == 0 {
                    lines.push(Line::from(format!("{role}: {part}")));
                } else {
                    lines.push(Line::from(part.to_string()));
                }
            }
        }
    }
    lines
}

/// Maximum inner rows the input box grows to before it scrolls internally.
const MAX_INPUT_ROWS: u16 = 6;

/// Draw the three panes.
pub fn render(frame: &mut Frame, app: &App) {
    // The input box grows with the composed line count (capped), then scrolls.
    let input_rows = u16::try_from(app.input.line_count())
        .unwrap_or(MAX_INPUT_ROWS)
        .clamp(1, MAX_INPUT_ROWS);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(input_rows + 2),
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

    // Render the (possibly multi-line) input, scrolling vertically and
    // horizontally just enough to keep the cursor inside the box, then park the
    // terminal cursor on it.
    let inner_w = chunks[1].width.saturating_sub(2) as usize;
    let inner_h = chunks[1].height.saturating_sub(2) as usize;
    let (crow, ccol) = app.input.cursor_row_col();
    let h_off = ccol.saturating_sub(inner_w.saturating_sub(1));
    let v_off = crow.saturating_sub(inner_h.saturating_sub(1));
    let input_lines: Vec<Line> = app
        .input
        .text()
        .split('\n')
        .map(|l| Line::from(l.to_string()))
        .collect();
    frame.render_widget(
        Paragraph::new(input_lines)
            .scroll((
                u16::try_from(v_off).unwrap_or(u16::MAX),
                u16::try_from(h_off).unwrap_or(u16::MAX),
            ))
            .block(Block::bordered().title(app.input_title())),
        chunks[1],
    );
    frame.set_cursor_position((
        chunks[1].x + 1 + u16::try_from(ccol - h_off).unwrap_or(u16::MAX),
        chunks[1].y + 1 + u16::try_from(crow - v_off).unwrap_or(u16::MAX),
    ));

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
        // Sending marks a turn in flight; the reply clears it before we test quit.
        assert!(app.turn_in_flight);
        app.finish_assistant("hi back".to_string());
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
    fn tracks_last_seq_and_dedups_replayed_events() {
        let mut app = App::new("ready");
        // An authoritative event advances the last-seen seq + conversation id.
        app.note_seq("c1", 5);
        assert_eq!(app.last_seq, 5);
        assert_eq!(app.last_conversation_id.as_deref(), Some("c1"));
        // A replay at or below the last seq is a duplicate — not re-inserted.
        let before = app.messages.len();
        assert!(!app.apply_replay("c1", 3, "assistant", "old"));
        assert_eq!(app.messages.len(), before, "duplicate not inserted");
        // A replay beyond the last seq is applied and advances the seq.
        assert!(app.apply_replay("c1", 6, "you", "new line"));
        assert_eq!(app.messages.len(), before + 1);
        assert_eq!(app.last_seq, 6);
        // Replaying that same seq again is now a duplicate.
        assert!(!app.apply_replay("c1", 6, "you", "new line"));
        assert_eq!(app.messages.len(), before + 1);
    }

    #[test]
    fn spinner_advances_while_waiting_and_is_quiet_when_idle() {
        let mut app = App::new("ready");
        // Idle: the input title shows no animated "Working…" indicator.
        assert!(!app.input_title().contains("Working"));
        // A turn in flight surfaces the current spinner glyph…
        app.turn_in_flight = true;
        let first = app.spinner_char();
        assert!(
            app.input_title().contains(first),
            "waiting title shows spinner"
        );
        // …which changes as the outer loop ticks it forward.
        app.advance_spinner();
        assert_ne!(first, app.spinner_char(), "frame advanced on tick");
        // Advancing all the way around wraps safely, never panicking.
        for _ in 0..=SPINNER.len() {
            app.advance_spinner();
        }
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
    fn assistant_code_block_is_visually_distinguished() {
        let mut app = App::new("ready");
        app.push("fleety", "intro line\n```\nlet x = 1;\n```\ndone");
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let rows = visible_rows(&terminal);
        // The code content survives and lands on a gutter-marked row ("│ code"),
        // set apart from the prose. (The pane border is also "│", so we assert
        // the gutter+content adjacency, not the bare glyph.)
        assert!(
            rows.iter().any(|r| r.contains("let x = 1;")),
            "code visible"
        );
        assert!(
            rows.iter().any(|r| r.contains("│ let x = 1;")),
            "code row carries the gutter marker"
        );
        // Prose renders with the role label, not a gutter.
        assert!(
            rows.iter().any(|r| r.contains("fleety: intro line")),
            "prose keeps the role label"
        );
        assert!(
            !rows.iter().any(|r| r.contains("│ intro line")),
            "prose is not gutter-marked"
        );
        assert!(
            rows.iter().any(|r| r.contains("done")),
            "trailing prose kept"
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
    fn alt_enter_and_ctrl_j_insert_newline_without_sending() {
        let mut app = App::new("ready");
        for c in "line1".chars() {
            on_key(&mut app, key(KeyCode::Char(c)));
        }
        // Alt+Enter inserts a break and does NOT submit.
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(on_key(&mut app, alt_enter), Action::None);
        assert!(app.input.text().contains('\n'));
        for c in "line2".chars() {
            on_key(&mut app, key(KeyCode::Char(c)));
        }
        // Ctrl+J is the compatibility newline key — also no submit.
        assert_eq!(on_key(&mut app, ctrl('j')), Action::None);
        assert_eq!(app.input.text(), "line1\nline2\n");
        // A bare Enter submits the whole multi-line buffer, newline included.
        let Action::Send { text, .. } = on_key(&mut app, key(KeyCode::Enter)) else {
            panic!("expected Send");
        };
        assert_eq!(text, "line1\nline2\n");
        assert!(text.contains('\n'), "embedded newline preserved");
    }

    #[test]
    fn multiline_input_renders_on_separate_rows() {
        let mut app = App::new("ready");
        app.input.set_text("aaa\nbbb".to_string());
        let mut terminal = Terminal::new(TestBackend::new(30, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let rows = visible_rows(&terminal);
        let r1 = rows.iter().position(|r| r.contains("aaa"));
        let r2 = rows.iter().position(|r| r.contains("bbb"));
        assert!(r1.is_some() && r2.is_some(), "both input lines visible");
        assert_ne!(r1, r2, "input lines on separate rows");
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
    fn attach_command_stages_file_without_sending_and_missing_reports_error() {
        let mut app = App::new("ready");
        let dir = std::env::temp_dir().join(format!("fleety-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk");
        let path = dir.join("note.txt");
        std::fs::write(&path, b"hello").expect("write");

        // A `/attach <existing>` submission stages the file and does NOT send.
        app.input
            .set_text(format!("/attach {}", path.to_string_lossy()));
        assert_eq!(on_key(&mut app, key(KeyCode::Enter)), Action::None);
        assert_eq!(app.pending_attachments.len(), 1);
        assert!(
            app.input.is_empty(),
            "input cleared after a successful attach"
        );

        // A missing path preserves the input and reports an error — nothing new
        // is staged and no message is sent.
        let missing = "/attach /no/such/file-xyz".to_string();
        app.input.set_text(missing.clone());
        assert_eq!(on_key(&mut app, key(KeyCode::Enter)), Action::None);
        assert_eq!(app.pending_attachments.len(), 1, "no new attachment staged");
        assert_eq!(app.input.text(), missing, "input preserved on failure");
        assert!(
            app.status.contains("could not attach"),
            "error reported: {}",
            app.status
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn esc_cancels_while_a_turn_is_in_flight_then_quits_when_idle() {
        let mut app = App::new("ready");
        // Sending a message marks a turn in flight.
        app.input.set_text("hello".into());
        let Action::Send { .. } = on_key(&mut app, key(KeyCode::Enter)) else {
            panic!("expected Send");
        };
        assert!(app.turn_in_flight);
        // Esc now cancels the turn (does NOT quit).
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::CancelTurn);
        assert!(!app.should_quit);
        assert!(app.status.contains("cancel"));
        // The reply landing clears the in-flight state.
        app.finish_assistant("done".into());
        assert!(!app.turn_in_flight);
        // With no turn in flight, Esc quits as before.
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn esc_with_unsent_input_confirms_before_quitting() {
        let mut app = App::new("ready");
        app.input.set_text("draft".into());
        // First Esc: enter the confirm state without quitting.
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::None);
        assert!(app.confirm_quit);
        assert!(!app.should_quit);
        // Second consecutive Esc: quit.
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn esc_with_pending_attachment_confirms() {
        let mut app = App::new("ready");
        app.attach(WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("AAAA".into()),
            url: None,
            name: None,
        });
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::None);
        assert!(app.confirm_quit, "pending attachment guards quit too");
    }

    #[test]
    fn editing_cancels_quit_confirmation() {
        let mut app = App::new("ready");
        app.input.set_text("draft".into());
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::None);
        assert!(app.confirm_quit);
        // An editing keypress clears the confirm state…
        on_key(&mut app, key(KeyCode::Char('!')));
        assert!(!app.confirm_quit);
        // …so the next Esc requires confirmation again rather than quitting.
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::None);
        assert!(!app.should_quit);
    }

    #[test]
    fn ctrl_c_bypasses_quit_confirmation() {
        let mut app = App::new("ready");
        app.input.set_text("draft".into());
        assert_eq!(on_key(&mut app, ctrl('c')), Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn esc_quits_directly_when_nothing_is_unsent() {
        let mut app = App::new("ready");
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
        assert!(!app.confirm_quit);
    }

    #[test]
    fn approval_deny_takes_priority_over_turn_cancel() {
        let mut app = App::new("ready");
        app.input.set_text("go".into());
        let _ = on_key(&mut app, key(KeyCode::Enter));
        assert!(app.turn_in_flight);
        // A mid-turn approval request: Esc is deny (modal wins over cancel).
        app.request_approval("id1".into(), "write_file", "mutate", "write foo");
        assert_eq!(
            on_key(&mut app, key(KeyCode::Esc)),
            Action::Deny("id1".into())
        );
        assert!(!app.should_quit);
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
