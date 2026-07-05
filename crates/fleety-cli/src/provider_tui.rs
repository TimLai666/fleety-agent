//! Interactive `config provider edit` screen (CLI-only; needs a TTY).
//!
//! Lists the providers, groups, and roles from `providers.toml` and edits them
//! in place: add/remove a provider, set a group's members + strategy, and bind a
//! role. Saving runs the same validation + atomic write as the `config
//! provider|group|role` subcommands (so the two paths can't diverge), and
//! provider keys are masked on screen.
//!
//! The state mutations live on [`ProviderEditor`] as small, pure methods that
//! are unit-tested; the ratatui render + key loop around them is thin and
//! verified by hand.

use std::path::Path;

use agent_core::{CoreError, Result};
use fleety_tools::providers_config::{
    self as pc, GroupSpec, ProviderSpec, ProvidersConfig, Strategy,
};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::input::LineEditor;

/// The editable `providers.toml` model. Mutations validate references lazily
/// (full validation runs on [`save`](Self::save)); the immediate guards here
/// give friendly errors for the common mistakes.
pub struct ProviderEditor {
    cfg: ProvidersConfig,
}

impl ProviderEditor {
    pub fn new(cfg: ProvidersConfig) -> Self {
        Self { cfg }
    }

    pub fn providers(&self) -> &[ProviderSpec] {
        &self.cfg.providers
    }

    /// Add a provider; a duplicate name is rejected.
    pub fn add_provider(
        &mut self,
        name: String,
        base_url: String,
        model: String,
        key: Option<String>,
    ) -> Result<()> {
        if self.cfg.provider(&name).is_some() {
            return Err(CoreError::Message(format!(
                "provider '{name}' already exists"
            )));
        }
        self.cfg.providers.push(ProviderSpec {
            name,
            base_url,
            model,
            key,
            stream: false,
            modalities: None,
            effort: None,
            auth: None,
        });
        Ok(())
    }

    /// Remove a provider; rejected if a group or role still references it.
    pub fn remove_provider(&mut self, name: &str) -> Result<()> {
        if self.cfg.provider(name).is_none() {
            return Err(CoreError::Message(format!("no such provider '{name}'")));
        }
        if let Some(g) = self
            .cfg
            .groups
            .iter()
            .find(|g| g.members.iter().any(|m| m == name))
        {
            return Err(CoreError::Message(format!(
                "group '{}' references provider '{name}'",
                g.name
            )));
        }
        if let Some((r, _)) = self.cfg.roles.iter().find(|(_, t)| t.as_str() == name) {
            return Err(CoreError::Message(format!(
                "role '{r}' references provider '{name}'"
            )));
        }
        self.cfg.providers.retain(|p| p.name != name);
        Ok(())
    }

    /// Create or replace a group.
    pub fn set_group(&mut self, name: String, members: Vec<String>, strategy: Strategy) {
        self.cfg.groups.retain(|g| g.name != name);
        self.cfg.groups.push(GroupSpec {
            name,
            members,
            strategy,
        });
    }

    /// Bind a role to a provider/group name.
    pub fn set_role(&mut self, role: String, target: String) {
        self.cfg.roles.insert(role, target);
    }

    /// Validate and write atomically (delegates to the shared writer).
    pub fn save(&self, path: &Path) -> Result<()> {
        pc::write_providers(path, &self.cfg)
    }
}

/// Parse a strategy word (shared shape with the subcommands).
fn strategy_word(s: &str) -> Result<Strategy> {
    match s.trim() {
        "round_robin" => Ok(Strategy::RoundRobin),
        "failover" => Ok(Strategy::Failover),
        other => Err(CoreError::Message(format!(
            "invalid strategy '{other}' (round_robin | failover)"
        ))),
    }
}

fn masked_key(key: &Option<String>) -> &'static str {
    match key {
        Some(k) if !k.is_empty() => "********",
        _ => "(none)",
    }
}

// ---- interactive screen ----

enum Action {
    AddProvider,
    SetRole,
    SetGroup,
}

enum Mode {
    Browse,
    Input {
        action: Action,
        prompt: &'static str,
        buffer: LineEditor,
    },
}

struct App {
    ed: ProviderEditor,
    sel: usize,
    mode: Mode,
    status: String,
    save_now: bool,
    quit: bool,
}

impl App {
    fn new(cfg: ProvidersConfig) -> Self {
        Self {
            ed: ProviderEditor::new(cfg),
            sel: 0,
            mode: Mode::Browse,
            status: "a add · d delete · g group · r role · s save · q quit".to_string(),
            save_now: false,
            quit: false,
        }
    }

    /// Apply the submitted input buffer for the active action.
    fn submit(&mut self, action: &Action, buf: &str) {
        let parts: Vec<String> = buf.split(',').map(|s| s.trim().to_string()).collect();
        let res = match action {
            Action::AddProvider => match parts.as_slice() {
                [name, base, model, rest @ ..] if !name.is_empty() => {
                    let key = rest.first().filter(|k| !k.is_empty()).cloned();
                    self.ed
                        .add_provider(name.clone(), base.clone(), model.clone(), key)
                }
                _ => Err(CoreError::Message(
                    "expected: name, base_url, model[, key]".to_string(),
                )),
            },
            Action::SetRole => match parts.as_slice() {
                [role, target] if !role.is_empty() && !target.is_empty() => {
                    self.ed.set_role(role.clone(), target.clone());
                    Ok(())
                }
                _ => Err(CoreError::Message("expected: role, target".to_string())),
            },
            Action::SetGroup => match parts.as_slice() {
                [name, members, strat] if !name.is_empty() => match strategy_word(strat) {
                    Ok(strategy) => {
                        let members = members
                            .split('|')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        self.ed.set_group(name.clone(), members, strategy);
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                _ => Err(CoreError::Message(
                    "expected: name, member1|member2, strategy".to_string(),
                )),
            },
        };
        self.status = match res {
            Ok(()) => "ok".to_string(),
            Err(e) => format!("error: {e}"),
        };
    }
}

fn on_key(app: &mut App, code: KeyCode) {
    match &mut app.mode {
        Mode::Input { buffer, .. } => match code {
            KeyCode::Char(c) => buffer.insert(c),
            KeyCode::Backspace => buffer.backspace(),
            KeyCode::Delete => buffer.delete(),
            KeyCode::Left => buffer.left(),
            KeyCode::Right => buffer.right(),
            KeyCode::Home => buffer.home(),
            KeyCode::End => buffer.end(),
            KeyCode::Esc => {
                app.mode = Mode::Browse;
                app.status = "cancelled".to_string();
            }
            KeyCode::Enter => {
                if let Mode::Input {
                    action, mut buffer, ..
                } = std::mem::replace(&mut app.mode, Mode::Browse)
                {
                    app.submit(&action, &buffer.take());
                }
            }
            _ => {}
        },
        Mode::Browse => match code {
            KeyCode::Char('q') => app.quit = true,
            KeyCode::Char('s') => app.save_now = true,
            KeyCode::Up => app.sel = app.sel.saturating_sub(1),
            KeyCode::Down => {
                let max = app.ed.providers().len().saturating_sub(1);
                app.sel = (app.sel + 1).min(max);
            }
            KeyCode::Char('a') => {
                app.mode = Mode::Input {
                    action: Action::AddProvider,
                    prompt: "add provider — name, base_url, model[, key]",
                    buffer: LineEditor::default(),
                };
            }
            KeyCode::Char('g') => {
                app.mode = Mode::Input {
                    action: Action::SetGroup,
                    prompt: "set group — name, member1|member2, strategy",
                    buffer: LineEditor::default(),
                };
            }
            KeyCode::Char('r') => {
                app.mode = Mode::Input {
                    action: Action::SetRole,
                    prompt: "set role — role, target",
                    buffer: LineEditor::default(),
                };
            }
            KeyCode::Char('d') => {
                let name = app.ed.providers().get(app.sel).map(|p| p.name.clone());
                match name {
                    Some(n) => {
                        app.status = match app.ed.remove_provider(&n) {
                            Ok(()) => format!("removed '{n}'"),
                            Err(e) => format!("error: {e}"),
                        };
                    }
                    None => app.status = "nothing selected".to_string(),
                }
            }
            _ => {}
        },
    }
}

fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(f.area());

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("Providers:"));
    for (i, p) in app.ed.cfg.providers.iter().enumerate() {
        let marker = if i == app.sel { "▶" } else { " " };
        lines.push(Line::from(format!(
            "{marker} {:<14} {} [{}] key={}",
            p.name,
            p.base_url,
            p.model,
            masked_key(&p.key)
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Groups:"));
    for g in &app.ed.cfg.groups {
        lines.push(Line::from(format!(
            "  {:<14} [{:?}] {}",
            g.name,
            g.strategy,
            g.members.join(", ")
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Roles:"));
    for (r, t) in &app.ed.cfg.roles {
        lines.push(Line::from(format!("  {r} → {t}")));
    }
    // Keep the selected provider row visible when the list outgrows the pane
    // (selection is line 1 + sel; line 0 is the "Providers:" header).
    let inner_h = chunks[0].height.saturating_sub(2);
    let sel_line = 1 + app.sel as u16;
    let offset = (sel_line + 1).saturating_sub(inner_h);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("providers.toml"))
            .scroll((offset, 0)),
        chunks[0],
    );

    // The footer's inner area is a single row, so the prompt goes in the block
    // title and the row shows the buffer being typed (with the cursor on it).
    match &app.mode {
        Mode::Browse => f.render_widget(
            Paragraph::new(app.status.clone()).block(Block::bordered()),
            chunks[1],
        ),
        Mode::Input { prompt, buffer, .. } => {
            // "> " takes two columns; the editor windows the rest so the
            // cursor stays visible when the value outgrows the footer.
            let max_w = chunks[1].width.saturating_sub(2) as usize;
            let (view, x) = buffer.display_window(max_w.saturating_sub(2));
            f.render_widget(
                Paragraph::new(format!("> {view}"))
                    .block(Block::bordered().title(format!("{prompt} · Enter save · Esc cancel"))),
                chunks[1],
            );
            f.set_cursor_position((
                chunks[1].x + 1 + (2 + x as usize).min(max_w) as u16,
                chunks[1].y + 1,
            ));
        }
    }
}

/// Open the interactive providers editor over `path`.
pub fn run(path: &Path) -> Result<()> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let cfg = pc::load_or_default(path)?;
    let mut app = App::new(cfg);
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        loop {
            terminal
                .draw(|f| render(f, &app))
                .map_err(|e| CoreError::Message(format!("draw failed: {e}")))?;
            if app.quit {
                break;
            }
            if let Event::Key(k) =
                event::read().map_err(|e| CoreError::Message(format!("read failed: {e}")))?
            {
                if k.kind != KeyEventKind::Release {
                    on_key(&mut app, k.code);
                    if app.save_now {
                        app.save_now = false;
                        app.status = match app.ed.save(path) {
                            Ok(()) => "saved".to_string(),
                            Err(e) => format!("save failed: {e}"),
                        };
                    }
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_dup_provider() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "u".into(), "m".into(), Some("sk".into()))
            .unwrap();
        assert_eq!(ed.providers().len(), 1);
        // Duplicate name is rejected.
        assert!(ed
            .add_provider("p1".into(), "u".into(), "m".into(), None)
            .is_err());
    }

    #[test]
    fn remove_guarded_by_group_reference() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "u".into(), "m".into(), None)
            .unwrap();
        ed.set_group("g".into(), vec!["p1".into()], Strategy::Failover);
        // Referenced by the group → cannot remove.
        assert!(ed
            .remove_provider("p1")
            .unwrap_err()
            .to_string()
            .contains("g"));
        // Unreferenced provider removes fine.
        ed.add_provider("p2".into(), "u".into(), "m".into(), None)
            .unwrap();
        ed.remove_provider("p2").unwrap();
        assert_eq!(ed.providers().len(), 1);
    }

    #[test]
    fn set_role_set_group_then_save_roundtrips() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "u".into(), "m".into(), None)
            .unwrap();
        ed.add_provider("p2".into(), "u".into(), "m".into(), None)
            .unwrap();
        ed.set_group(
            "g".into(),
            vec!["p1".into(), "p2".into()],
            Strategy::RoundRobin,
        );
        ed.set_role("main".into(), "g".into());
        let path = std::env::temp_dir().join(format!("fleety-tui-{}.toml", uuid::Uuid::new_v4()));
        ed.save(&path).expect("save");
        let back = pc::load_from(&path).expect("re-read");
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.roles.get("main").map(String::as_str), Some("g"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn input_mode_shows_the_typed_buffer() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // enter add-provider input mode
        for c in "abc".chars() {
            on_key(&mut app, KeyCode::Char(c));
        }
        // Cursor editing works mid-buffer (Left + insert, not append).
        on_key(&mut app, KeyCode::Left);
        on_key(&mut app, KeyCode::Char('X'));
        let mut terminal = Terminal::new(TestBackend::new(70, 10)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // The buffer being typed must be visible (it used to be clipped by the
        // 3-row footer whose inner area is a single row).
        assert!(content.contains("> abXc"), "typed buffer visible");
        assert!(content.contains("add provider"), "prompt visible in title");
    }

    #[test]
    fn save_rejects_dangling_role() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        // A role pointing nowhere fails validation on save.
        ed.set_role("main".into(), "ghost".into());
        let path = std::env::temp_dir().join(format!("fleety-tui-{}.toml", uuid::Uuid::new_v4()));
        assert!(ed.save(&path).is_err());
        assert!(!path.exists());
    }
}
