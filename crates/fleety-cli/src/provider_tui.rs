//! Interactive `config provider edit` screen (CLI-only; needs a TTY).
//!
//! Lists the type-tagged providers and the `main`/`cheap` model roles from
//! `providers.toml` and edits them in place via single-line inputs: add/remove a
//! provider (by `type`), set a model role's members + strategy, and unset a
//! role. Saving runs the same validation + atomic write as the non-interactive
//! `config provider|model` subcommands (so the two paths can't diverge), and
//! provider keys are masked on screen.
//!
//! The state mutations live on [`ProviderEditor`] as small, pure methods that
//! are unit-tested; the ratatui render + key loop around them is thin. (Design
//! note: this is the minimal-viable two-tier editor — per-field forms and
//! in-place provider editing from the old single-tier UI are dropped; use
//! remove+add, or the non-interactive commands, for those.)

use std::path::Path;

use agent_core::{CoreError, Result};
use fleety_tools::providers_config::{
    self as pc, Member, ModelPool, Provider, ProvidersConfig, Strategy,
};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::input::LineEditor;

/// The editable two-tier `providers.toml` model. Full validation runs on
/// [`save`](Self::save); the immediate guards here give friendly errors for the
/// common mistakes (dup provider, removing a referenced provider).
pub struct ProviderEditor {
    cfg: ProvidersConfig,
}

impl ProviderEditor {
    pub fn new(cfg: ProvidersConfig) -> Self {
        Self { cfg }
    }

    /// The provider names in display order (BTreeMap → sorted).
    pub fn provider_names(&self) -> Vec<String> {
        self.cfg.providers.keys().cloned().collect()
    }

    /// Add a type-tagged provider; a duplicate name is rejected. Type field
    /// rules (api needs base_url; oauth carries none) are enforced at [`save`].
    pub fn add_provider(
        &mut self,
        name: String,
        kind: String,
        base_url: Option<String>,
        key: Option<String>,
    ) -> Result<()> {
        if self.cfg.providers.contains_key(&name) {
            return Err(CoreError::Message(format!(
                "provider '{name}' already exists"
            )));
        }
        self.cfg.providers.insert(
            name,
            Provider {
                kind,
                base_url,
                key,
            },
        );
        Ok(())
    }

    /// Remove a provider; rejected if a model role member still references it
    /// (the error names the role).
    pub fn remove_provider(&mut self, name: &str) -> Result<()> {
        if !self.cfg.providers.contains_key(name) {
            return Err(CoreError::Message(format!("no such provider '{name}'")));
        }
        if let Some(role) = self.cfg.role_referencing(name) {
            return Err(CoreError::Message(format!(
                "model role '{role}' references provider '{name}'"
            )));
        }
        self.cfg.providers.remove(name);
        Ok(())
    }

    /// Create or replace a model role's member pool.
    pub fn set_model(&mut self, role: String, members: Vec<Member>, strategy: Strategy) {
        self.cfg
            .models
            .insert(role, ModelPool { strategy, members });
    }

    /// Unset a model role; an undefined role is reported by name.
    pub fn unset_model(&mut self, role: &str) -> Result<()> {
        if self.cfg.models.remove(role).is_none() {
            return Err(CoreError::Message(format!("no such model role '{role}'")));
        }
        Ok(())
    }

    /// The current edited configuration (for savers that ship it elsewhere,
    /// e.g. the remote apply).
    pub fn config(&self) -> &ProvidersConfig {
        &self.cfg
    }
}

/// What a save attempt did. The remote saver reports a concurrent-edit
/// conflict as data (not an error) so the editor can exit and the caller
/// reload from a fresh snapshot instead of overwriting.
pub enum SaveOutcome {
    Saved,
    Conflict(String),
}

/// Parse a strategy word (shared shape with the subcommands).
fn strategy_word(s: &str) -> Result<Strategy> {
    match s.trim() {
        "single" => Ok(Strategy::Single),
        "round_robin" => Ok(Strategy::RoundRobin),
        "failover" => Ok(Strategy::Failover),
        other => Err(CoreError::Message(format!(
            "invalid strategy '{other}' (single | round_robin | failover)"
        ))),
    }
}

/// Parse `p1/m1|p2/m2` into members.
fn parse_members(s: &str) -> Result<Vec<Member>> {
    let mut out = Vec::new();
    for spec in s.split('|').map(str::trim).filter(|s| !s.is_empty()) {
        let (provider, model) = spec.split_once('/').ok_or_else(|| {
            CoreError::Message(format!("member '{spec}' must be <provider>/<model>"))
        })?;
        out.push(Member {
            provider: provider.to_string(),
            model: model.to_string(),
            stream: false,
            modalities: None,
            effort: None,
        });
    }
    if out.is_empty() {
        return Err(CoreError::Message(
            "need at least one member <provider>/<model>".to_string(),
        ));
    }
    Ok(out)
}

fn masked_key(key: &Option<String>) -> &'static str {
    match key {
        Some(k) if !k.is_empty() => "********",
        _ => "(none)",
    }
}

// ---- interactive screen ----

/// A single-line action collecting one comma-separated input line.
enum Action {
    AddProvider,
    SetModel,
    UnsetModel,
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
            status:
                "a add-provider · d del-provider · m set-model · u unset-model · s save · q quit"
                    .to_string(),
            save_now: false,
            quit: false,
        }
    }

    /// Apply the submitted input buffer for the active single-line action.
    fn submit(&mut self, action: &Action, buf: &str) {
        let parts: Vec<String> = buf.split(',').map(|s| s.trim().to_string()).collect();
        let res = match action {
            Action::AddProvider => match parts.as_slice() {
                // name, type[, base_url[, key]]
                [name, kind, rest @ ..] if !name.is_empty() && !kind.is_empty() => {
                    let base_url = rest.first().filter(|s| !s.is_empty()).cloned();
                    let key = rest.get(1).filter(|s| !s.is_empty()).cloned();
                    self.ed
                        .add_provider(name.clone(), kind.clone(), base_url, key)
                }
                _ => Err(CoreError::Message(
                    "expected: name, type [, base_url [, key]]".to_string(),
                )),
            },
            Action::SetModel => match parts.as_slice() {
                // role, p1/m1|p2/m2, strategy
                [role, members, strat] if !role.is_empty() => {
                    match (parse_members(members), strategy_word(strat)) {
                        (Ok(members), Ok(strategy)) => {
                            self.ed.set_model(role.clone(), members, strategy);
                            Ok(())
                        }
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    }
                }
                _ => Err(CoreError::Message(
                    "expected: role, p1/m1|p2/m2, strategy".to_string(),
                )),
            },
            Action::UnsetModel => match parts.as_slice() {
                [role] if !role.is_empty() => self.ed.unset_model(role),
                _ => Err(CoreError::Message("expected: model role".to_string())),
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
                let max = app.ed.provider_names().len().saturating_sub(1);
                app.sel = (app.sel + 1).min(max);
            }
            KeyCode::Char('a') => {
                app.mode = Mode::Input {
                    action: Action::AddProvider,
                    prompt: "add provider — name, type(api|oauth:codex) [, base_url [, key]]",
                    buffer: LineEditor::default(),
                };
            }
            KeyCode::Char('d') => match app.ed.provider_names().get(app.sel).cloned() {
                Some(name) => {
                    app.status = match app.ed.remove_provider(&name) {
                        Ok(()) => format!("removed '{name}'"),
                        Err(e) => format!("error: {e}"),
                    };
                }
                None => app.status = "nothing selected".to_string(),
            },
            KeyCode::Char('m') => {
                app.mode = Mode::Input {
                    action: Action::SetModel,
                    prompt: "set model — role, p1/m1|p2/m2, strategy",
                    buffer: LineEditor::default(),
                };
            }
            KeyCode::Char('u') => {
                app.mode = Mode::Input {
                    action: Action::UnsetModel,
                    prompt: "unset model — role",
                    buffer: LineEditor::default(),
                };
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
    for (i, (name, p)) in app.ed.cfg.providers.iter().enumerate() {
        let marker = if i == app.sel { "▶" } else { " " };
        let endpoint = p.base_url.as_deref().unwrap_or("(oauth login)");
        lines.push(Line::from(format!(
            "{marker} {:<14} [{}] {} key={}",
            name,
            p.kind,
            endpoint,
            masked_key(&p.key)
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Model roles:"));
    for (role, pool) in &app.ed.cfg.models {
        let members = pool
            .members
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.model))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(format!(
            "  {:<8} [{:?}] {}",
            role, pool.strategy, members
        )));
    }
    let inner_h = chunks[0].height.saturating_sub(2);
    let sel_line = 1 + app.sel as u16;
    let offset = (sel_line + 1).saturating_sub(inner_h);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("providers.toml"))
            .scroll((offset, 0)),
        chunks[0],
    );

    match &app.mode {
        Mode::Browse => f.render_widget(
            Paragraph::new(app.status.clone()).block(Block::bordered()),
            chunks[1],
        ),
        Mode::Input { prompt, buffer, .. } => {
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

/// Open the interactive providers editor over `path` (this host's own file).
pub fn run(path: &Path) -> Result<()> {
    let cfg = pc::load_or_default(path)?;
    run_with_saver(cfg, |edited| {
        pc::write_providers(path, edited).map(|()| SaveOutcome::Saved)
    })
    .map(|_| ())
}

/// Open the interactive providers editor over an in-memory configuration; every
/// save goes through `save` (local file write or remote apply — the editor does
/// not care). Returns the conflict message when the editor exited because a
/// save hit a concurrent-edit conflict (the caller reloads and reopens), `None`
/// on a normal quit.
pub fn run_with_saver(
    cfg: ProvidersConfig,
    mut save: impl FnMut(&ProvidersConfig) -> Result<SaveOutcome>,
) -> Result<Option<String>> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut app = App::new(cfg);
    let mut conflict: Option<String> = None;
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
                        match save(app.ed.config()) {
                            Ok(SaveOutcome::Saved) => app.status = "saved".to_string(),
                            Ok(SaveOutcome::Conflict(msg)) => {
                                // Someone else changed the target while editing:
                                // never overwrite — leave, let the caller reload.
                                conflict = Some(msg);
                                app.quit = true;
                            }
                            Err(e) => app.status = format!("save failed: {e}"),
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result.map(|()| conflict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(base: &str) -> (String, Option<String>, Option<String>) {
        ("api".to_string(), Some(base.to_string()), None)
    }

    #[test]
    fn add_and_dup_provider() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        let (kind, base, key) = api("https://u/v1");
        ed.add_provider("p1".into(), kind, base, key).unwrap();
        assert_eq!(ed.provider_names(), vec!["p1".to_string()]);
        // Duplicate name is rejected.
        assert!(ed
            .add_provider("p1".into(), "api".into(), Some("https://x/v1".into()), None)
            .is_err());
    }

    #[test]
    fn remove_guarded_by_model_reference() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        ed.set_model(
            "main".into(),
            vec![Member {
                provider: "p1".into(),
                model: "gpt-4o".into(),
                stream: false,
                modalities: None,
                effort: None,
            }],
            Strategy::Single,
        );
        // Referenced by the main role → cannot remove (error names the role).
        assert!(ed
            .remove_provider("p1")
            .unwrap_err()
            .to_string()
            .contains("main"));
        // Unreferenced provider removes fine.
        ed.add_provider(
            "p2".into(),
            "api".into(),
            Some("https://u2/v1".into()),
            None,
        )
        .unwrap();
        ed.remove_provider("p2").unwrap();
        assert_eq!(ed.provider_names(), vec!["p1".to_string()]);
    }

    #[test]
    fn set_model_then_save_roundtrips() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        ed.set_model(
            "main".into(),
            parse_members("p1/gpt-4o|p1/gpt-4o-2").unwrap(),
            Strategy::RoundRobin,
        );
        let path = std::env::temp_dir().join(format!("fleety-tui-{}.toml", uuid::Uuid::new_v4()));
        pc::write_providers(&path, ed.config()).expect("save");
        let back = pc::load_from(&path).expect("re-read");
        assert_eq!(back.model("main").unwrap().members.len(), 2);
        assert_eq!(back.model("main").unwrap().strategy, Strategy::RoundRobin);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_rejects_dangling_member() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        // A model member referencing an undefined provider fails validation on save.
        ed.set_model(
            "main".into(),
            parse_members("ghost/m").unwrap(),
            Strategy::Single,
        );
        let path = std::env::temp_dir().join(format!("fleety-tui-{}.toml", uuid::Uuid::new_v4()));
        assert!(pc::write_providers(&path, ed.config()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn unset_model_removes_and_rejects_unknown() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        ed.set_model(
            "main".into(),
            parse_members("p1/gpt-4o").unwrap(),
            Strategy::Single,
        );
        ed.unset_model("main").unwrap();
        assert!(ed.cfg.model("main").is_none());
        assert!(ed
            .unset_model("ghost")
            .unwrap_err()
            .to_string()
            .contains("ghost"));
    }

    /// Feed each char of `s` to the app as a key press (input typing helper).
    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            on_key(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn add_provider_via_input_line() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // enter add-provider input mode
        type_str(&mut app, "openai1, api, https://u/v1, sk");
        on_key(&mut app, KeyCode::Enter);
        assert_eq!(app.ed.provider_names(), vec!["openai1".to_string()]);
        let p = app.ed.cfg.provider("openai1").unwrap();
        assert_eq!(p.kind, "api");
        assert_eq!(p.base_url.as_deref(), Some("https://u/v1"));
        assert_eq!(p.key.as_deref(), Some("sk"));
    }

    #[test]
    fn set_model_via_input_line() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        on_key(&mut app, KeyCode::Char('m'));
        type_str(&mut app, "main, p1/gpt-4o|p1/gpt-4o-2, failover");
        on_key(&mut app, KeyCode::Enter);
        assert_eq!(app.ed.cfg.model("main").unwrap().members.len(), 2);
        assert_eq!(
            app.ed.cfg.model("main").unwrap().strategy,
            Strategy::Failover
        );
    }

    #[test]
    fn input_mode_shows_the_typed_buffer() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a'));
        for c in "abc".chars() {
            on_key(&mut app, KeyCode::Char(c));
        }
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
        assert!(content.contains("> abXc"), "typed buffer visible");
        assert!(content.contains("add provider"), "prompt visible in title");
    }
}
