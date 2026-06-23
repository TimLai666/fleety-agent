//! Interactive multi-pane TUI (ratatui): a conversation pane, an input box, and
//! a status line. The `App` state, key handling, and rendering are unit-tested
//! (ratatui `TestBackend` renders to an in-memory buffer); the live
//! terminal/event/WebSocket loop in `main.rs` is the glue around them.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
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
}

/// What a keypress asks the outer loop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Send(String),
    Quit,
}

/// Apply a keypress to the app, returning the action for the loop to perform.
pub fn on_key(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.should_quit = true;
            Action::Quit
        }
        KeyCode::Enter => {
            let text = std::mem::take(&mut app.input);
            if text.trim().is_empty() {
                Action::None
            } else {
                app.push("you", text.clone());
                Action::Send(text)
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
        Paragraph::new(app.input.as_str())
            .block(Block::bordered().title("Message (Enter=send, Esc=quit)")),
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
            Action::Send("h".to_string())
        );
        assert_eq!(app.input, ""); // cleared on send
        assert_eq!(app.messages.last().map(|(r, _)| r.as_str()), Some("you"));
        // Empty Enter does nothing.
        assert_eq!(on_key(&mut app, key(KeyCode::Enter)), Action::None);
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
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
        let mut terminal = Terminal::new(TestBackend::new(50, 12)).expect("term");
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
