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

use fleety_protocol::WireAttachment;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

/// TUI state.
pub struct App {
    pub messages: Vec<(String, String)>,
    pub input: String,
    pub status: String,
    pub should_quit: bool,
    /// Attachments staged for the next `Send`. Cleared automatically when the
    /// user submits the line.
    pub pending_attachments: Vec<WireAttachment>,
    /// Whether the last message is an assistant reply still streaming in.
    streaming: bool,
}

impl App {
    pub fn new(status: impl Into<String>) -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            status: status.into(),
            should_quit: false,
            pending_attachments: Vec::new(),
            streaming: false,
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
        if self.pending_attachments.is_empty() {
            "Message (Enter=send, Ctrl+V=paste, Esc=quit)".to_string()
        } else {
            format!(
                "Message [{} attached] (Enter=send, Ctrl+X=drop attachments, Esc=quit)",
                self.pending_attachments.len()
            )
        }
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
}

/// Apply a keypress to the app, returning the action for the loop to perform.
pub fn on_key(app: &mut App, key: KeyEvent) -> Action {
    // Ctrl-prefixed shortcuts come first so plain Char fallthrough doesn't
    // swallow them (e.g. Ctrl+V should NOT type 'v').
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            match c.to_ascii_lowercase() {
                'v' => return Action::PasteFromClipboard,
                'x' => {
                    app.clear_attachments();
                    return Action::None;
                }
                'c' => {
                    app.should_quit = true;
                    return Action::Quit;
                }
                _ => {}
            }
        }
    }
    match key.code {
        KeyCode::Esc => {
            app.should_quit = true;
            Action::Quit
        }
        KeyCode::Enter => {
            let text = std::mem::take(&mut app.input);
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
                Action::Send { text, attachments }
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
            Action::None
        }
        KeyCode::Char(c) => {
            app.input.push(c);
            Action::None
        }
        _ => Action::None,
    }
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

    let lines: Vec<Line> = app
        .messages
        .iter()
        .map(|(role, text)| Line::from(format!("{role}: {text}")))
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Fleety"))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(app.input.as_str()).block(Block::bordered().title(app.input_title())),
        chunks[1],
    );
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
        assert_eq!(app.input, "hi");
        on_key(&mut app, key(KeyCode::Backspace));
        assert_eq!(app.input, "h");
        assert_eq!(
            on_key(&mut app, key(KeyCode::Enter)),
            Action::Send {
                text: "h".to_string(),
                attachments: Vec::new(),
            }
        );
        assert_eq!(app.input, ""); // cleared on send
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
        app.input = "typing".into();
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
}
