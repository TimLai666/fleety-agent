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

    /// Update an existing provider in place, reusing the `config provider set`
    /// semantics: only the fields given as `Some` change, the rest are kept (a
    /// `None` `key` preserves the current key). A missing provider is rejected
    /// by name. Because the identity (`name`) is untouched, a provider a group
    /// or role references can be edited without first unbinding it.
    pub fn set_provider(
        &mut self,
        name: &str,
        base_url: Option<String>,
        model: Option<String>,
        key: Option<String>,
    ) -> Result<()> {
        let p = self
            .cfg
            .providers
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| CoreError::Message(format!("no such provider '{name}'")))?;
        if let Some(v) = base_url {
            p.base_url = v;
        }
        if let Some(v) = model {
            p.model = v;
        }
        if key.is_some() {
            p.key = key;
        }
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

    /// Remove a group, matching `config group remove`: a group that a role still
    /// targets is rejected (naming the referring role) and left in place; a
    /// missing group is rejected by name.
    pub fn remove_group(&mut self, name: &str) -> Result<()> {
        if self.cfg.group(name).is_none() {
            return Err(CoreError::Message(format!("no such group '{name}'")));
        }
        if let Some((r, _)) = self.cfg.roles.iter().find(|(_, t)| t.as_str() == name) {
            return Err(CoreError::Message(format!(
                "cannot remove group '{name}': role '{r}' references it"
            )));
        }
        self.cfg.groups.retain(|g| g.name != name);
        Ok(())
    }

    /// Bind a role to a provider/group name.
    pub fn set_role(&mut self, role: String, target: String) {
        self.cfg.roles.insert(role, target);
    }

    /// Unset a role binding, matching `config role unset`: an undefined role is
    /// reported by name and the config is left unchanged.
    pub fn unset_role(&mut self, role: &str) -> Result<()> {
        if self.cfg.roles.remove(role).is_none() {
            return Err(CoreError::Message(format!("no such role '{role}'")));
        }
        Ok(())
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

/// Single-line actions that still take one comma-separated line (the provider
/// add/edit flow moved to the per-field [`ProviderForm`]).
enum Action {
    SetRole,
    SetGroup,
    RemoveGroup,
    UnsetRole,
}

/// Which field the per-field provider entry is collecting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    BaseUrl,
    Model,
    Key,
}

impl Field {
    /// The canonical field name shown in prompts and error messages.
    fn label(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::BaseUrl => "base_url",
            Field::Model => "model",
            Field::Key => "key",
        }
    }

    /// name/base_url/model must be non-empty; the key is optional.
    fn required(self) -> bool {
        !matches!(self, Field::Key)
    }
}

/// Result of committing the current field of a [`ProviderForm`].
enum FieldOutcome {
    /// Advanced to the next field (form stays open).
    Continue,
    /// A required field was left empty; the message names it (form stays open).
    Invalid(String),
    /// The last field was committed; the form is ready to apply.
    Done,
}

/// Per-field entry for adding a new provider or editing an existing one. When
/// `editing` is `Some(name)` the provider is updated in place (the name is fixed
/// so the Name field is skipped and base_url/model are prefilled); `None` adds a
/// new one. The key is never prefilled — leaving it blank on an edit keeps the
/// current key (matching the `set_provider` "only given fields change" rule).
struct ProviderForm {
    editing: Option<String>,
    field: Field,
    name: String,
    base_url: String,
    model: String,
    key: String,
    buffer: LineEditor,
}

impl ProviderForm {
    /// Start adding a brand-new provider (all fields empty, begins at Name).
    fn add() -> Self {
        Self {
            editing: None,
            field: Field::Name,
            name: String::new(),
            base_url: String::new(),
            model: String::new(),
            key: String::new(),
            buffer: LineEditor::default(),
        }
    }

    /// Start editing `p` in place: name fixed, base_url/model prefilled, key
    /// blank. Begins at the base_url field.
    fn edit(p: &ProviderSpec) -> Self {
        let mut buffer = LineEditor::default();
        buffer.set_text(p.base_url.clone());
        Self {
            editing: Some(p.name.clone()),
            field: Field::BaseUrl,
            name: p.name.clone(),
            base_url: p.base_url.clone(),
            model: p.model.clone(),
            key: String::new(),
            buffer,
        }
    }

    /// The footer prompt for the field currently being collected.
    fn prompt(&self) -> String {
        let verb = match &self.editing {
            Some(name) => format!("edit '{name}'"),
            None => "add provider".to_string(),
        };
        let hint = match self.field {
            Field::Key if self.editing.is_some() => "key (blank keeps current)".to_string(),
            Field::Key => "key (optional)".to_string(),
            f => f.label().to_string(),
        };
        format!("{verb} — {hint}")
    }

    /// Store the current buffer into its slot and advance. A required field left
    /// empty is rejected by name and the form stays on it.
    fn commit_field(&mut self) -> FieldOutcome {
        let value = self.buffer.take().trim().to_string();
        if self.field.required() && value.is_empty() {
            return FieldOutcome::Invalid(format!("{} is required", self.field.label()));
        }
        match self.field {
            Field::Name => self.name = value,
            Field::BaseUrl => self.base_url = value,
            Field::Model => self.model = value,
            Field::Key => {
                self.key = value;
                return FieldOutcome::Done;
            }
        }
        // Advance to the next field, prefilling its buffer from the slot (empty
        // for a fresh add, the current value for an edit).
        let next = match self.field {
            Field::Name => Field::BaseUrl,
            Field::BaseUrl => Field::Model,
            // The Key arm returned Done above; mapping it to itself is a
            // never-taken, never-panic fallback.
            Field::Model | Field::Key => Field::Key,
        };
        self.field = next;
        let prefill = match next {
            Field::BaseUrl => self.base_url.clone(),
            Field::Model => self.model.clone(),
            Field::Name | Field::Key => self.key.clone(),
        };
        self.buffer.set_text(prefill);
        FieldOutcome::Continue
    }
}

enum Mode {
    Browse,
    Input {
        action: Action,
        prompt: &'static str,
        buffer: LineEditor,
    },
    /// Per-field add/edit provider entry.
    Form(ProviderForm),
    /// Confirmation guard before deleting a provider.
    ConfirmDelete {
        name: String,
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
            status: "a add · e edit · d del · g group · G rm-group · r role · R unset-role · s save · q quit"
                .to_string(),
            save_now: false,
            quit: false,
        }
    }

    /// Apply a completed per-field provider form (add or edit-in-place).
    fn apply_provider_form(&mut self, form: &ProviderForm) {
        // A blank key means "none" on add and "keep current" on edit; both map
        // to `None` so `set_provider`/`add_provider` do the right thing.
        let key = if form.key.is_empty() {
            None
        } else {
            Some(form.key.clone())
        };
        let res = match &form.editing {
            Some(name) => self.ed.set_provider(
                name,
                Some(form.base_url.clone()),
                Some(form.model.clone()),
                key,
            ),
            None => self.ed.add_provider(
                form.name.clone(),
                form.base_url.clone(),
                form.model.clone(),
                key,
            ),
        };
        self.status = match res {
            Ok(()) => match &form.editing {
                Some(name) => format!("updated '{name}'"),
                None => format!("added '{}'", form.name),
            },
            Err(e) => format!("error: {e}"),
        };
    }

    /// Apply the submitted input buffer for the active single-line action.
    fn submit(&mut self, action: &Action, buf: &str) {
        let parts: Vec<String> = buf.split(',').map(|s| s.trim().to_string()).collect();
        let res = match action {
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
            Action::RemoveGroup => match parts.as_slice() {
                [name] if !name.is_empty() => self.ed.remove_group(name),
                _ => Err(CoreError::Message("expected: group name".to_string())),
            },
            Action::UnsetRole => match parts.as_slice() {
                [role] if !role.is_empty() => self.ed.unset_role(role),
                _ => Err(CoreError::Message("expected: role name".to_string())),
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
        Mode::Form(form) => match code {
            KeyCode::Char(c) => form.buffer.insert(c),
            KeyCode::Backspace => form.buffer.backspace(),
            KeyCode::Delete => form.buffer.delete(),
            KeyCode::Left => form.buffer.left(),
            KeyCode::Right => form.buffer.right(),
            KeyCode::Home => form.buffer.home(),
            KeyCode::End => form.buffer.end(),
            KeyCode::Esc => {
                app.mode = Mode::Browse;
                app.status = "cancelled".to_string();
            }
            KeyCode::Enter => match form.commit_field() {
                FieldOutcome::Continue => {}
                FieldOutcome::Invalid(msg) => app.status = format!("error: {msg}"),
                FieldOutcome::Done => {
                    // The last field is in; take the form out and apply it.
                    if let Mode::Form(form) = std::mem::replace(&mut app.mode, Mode::Browse) {
                        app.apply_provider_form(&form);
                    }
                }
            },
            _ => {}
        },
        Mode::ConfirmDelete { .. } => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Mode::ConfirmDelete { name } =
                    std::mem::replace(&mut app.mode, Mode::Browse)
                {
                    app.status = match app.ed.remove_provider(&name) {
                        Ok(()) => format!("removed '{name}'"),
                        Err(e) => format!("error: {e}"),
                    };
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.mode = Mode::Browse;
                app.status = "cancelled".to_string();
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
            KeyCode::Char('a') => app.mode = Mode::Form(ProviderForm::add()),
            KeyCode::Char('e') => match app.ed.providers().get(app.sel).cloned() {
                Some(p) => app.mode = Mode::Form(ProviderForm::edit(&p)),
                None => app.status = "nothing selected".to_string(),
            },
            KeyCode::Char('g') => {
                app.mode = Mode::Input {
                    action: Action::SetGroup,
                    prompt: "set group — name, member1|member2, strategy",
                    buffer: LineEditor::default(),
                };
            }
            KeyCode::Char('G') => {
                app.mode = Mode::Input {
                    action: Action::RemoveGroup,
                    prompt: "remove group — name",
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
            KeyCode::Char('R') => {
                app.mode = Mode::Input {
                    action: Action::UnsetRole,
                    prompt: "unset role — role",
                    buffer: LineEditor::default(),
                };
            }
            KeyCode::Char('d') => match app.ed.providers().get(app.sel).map(|p| p.name.clone()) {
                Some(name) => app.mode = Mode::ConfirmDelete { name },
                None => app.status = "nothing selected".to_string(),
            },
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
        Mode::Form(form) => {
            // Same windowed single-row input as `Mode::Input`, but the prompt
            // and the Enter hint reflect the current per-field step.
            let max_w = chunks[1].width.saturating_sub(2) as usize;
            let (view, x) = form.buffer.display_window(max_w.saturating_sub(2));
            let enter_hint = if form.field == Field::Key {
                "Enter save"
            } else {
                "Enter next"
            };
            f.render_widget(
                Paragraph::new(format!("> {view}")).block(
                    Block::bordered().title(format!("{} · {enter_hint} · Esc cancel", form.prompt())),
                ),
                chunks[1],
            );
            f.set_cursor_position((
                chunks[1].x + 1 + (2 + x as usize).min(max_w) as u16,
                chunks[1].y + 1,
            ));
        }
        Mode::ConfirmDelete { name } => f.render_widget(
            Paragraph::new(format!("delete provider '{name}'? · y confirm · n/Esc cancel"))
                .block(Block::bordered().title("confirm delete")),
            chunks[1],
        ),
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

    #[test]
    fn set_provider_edits_in_place_preserving_unchanged_and_bindings() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "u".into(), "m".into(), Some("sk".into()))
            .unwrap();
        ed.set_group("g".into(), vec!["p1".into()], Strategy::Failover);
        ed.set_role("main".into(), "g".into());
        // Change only the model; base_url and key are None → preserved (same
        // "only the given fields change" semantics as `config provider set`).
        ed.set_provider("p1", None, Some("m2".into()), None).unwrap();
        let p = ed.cfg.provider("p1").unwrap();
        assert_eq!(p.model, "m2");
        assert_eq!(p.base_url, "u"); // unchanged
        assert_eq!(p.key.as_deref(), Some("sk")); // key None = preserve
        // The group (and its member binding) is untouched by the edit.
        assert_eq!(ed.cfg.group("g").unwrap().members, vec!["p1".to_string()]);
        // Editing a missing provider is rejected by name.
        assert!(ed
            .set_provider("ghost", Some("x".into()), None, None)
            .unwrap_err()
            .to_string()
            .contains("ghost"));
    }

    #[test]
    fn set_provider_changes_only_key_when_others_untouched() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "u".into(), "m".into(), Some("old".into()))
            .unwrap();
        // The edit UI prefills base_url and model, so it passes them back as
        // their current values while swapping the key — result keeps them.
        ed.set_provider("p1", Some("u".into()), Some("m".into()), Some("new".into()))
            .unwrap();
        let p = ed.cfg.provider("p1").unwrap();
        assert_eq!(p.base_url, "u");
        assert_eq!(p.model, "m");
        assert_eq!(p.key.as_deref(), Some("new"));
    }

    #[test]
    fn remove_group_guarded_by_role_reference() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "u".into(), "m".into(), None)
            .unwrap();
        ed.set_group("g".into(), vec!["p1".into()], Strategy::Failover);
        ed.set_role("main".into(), "g".into());
        // A role still targets the group → removal is rejected, naming the role.
        let err = ed.remove_group("g").unwrap_err().to_string();
        assert!(err.contains("main"), "names the referring role: {err}");
        assert!(ed.cfg.group("g").is_some(), "group left in place");
        // Unbind the role, then removal succeeds.
        ed.unset_role("main").unwrap();
        ed.remove_group("g").unwrap();
        assert!(ed.cfg.group("g").is_none());
        // Removing a missing group is rejected by name.
        assert!(ed
            .remove_group("nope")
            .unwrap_err()
            .to_string()
            .contains("nope"));
    }

    #[test]
    fn unset_role_removes_binding_and_rejects_unknown() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "u".into(), "m".into(), None)
            .unwrap();
        ed.set_role("main".into(), "p1".into());
        ed.unset_role("main").unwrap();
        assert!(!ed.cfg.roles.contains_key("main"));
        // Unsetting an undefined role is reported (no such role) by name.
        assert!(ed
            .unset_role("ghost")
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
    fn per_field_add_rejects_empty_model_then_completes() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // add → Name field
        type_str(&mut app, "p1");
        on_key(&mut app, KeyCode::Enter); // → base_url
        type_str(&mut app, "u");
        on_key(&mut app, KeyCode::Enter); // → model
        // Leave the (required) model empty: rejected by name, nothing added.
        on_key(&mut app, KeyCode::Enter);
        assert!(
            app.status.contains("model"),
            "empty model rejected by name: {}",
            app.status
        );
        assert!(app.ed.providers().is_empty(), "no provider added yet");
        // Supply the model and finish (blank key → None).
        type_str(&mut app, "m");
        on_key(&mut app, KeyCode::Enter); // → key
        on_key(&mut app, KeyCode::Enter); // blank key → finalize add
        assert_eq!(app.ed.providers().len(), 1);
        let p = &app.ed.providers()[0];
        assert_eq!(p.name, "p1");
        assert_eq!(p.base_url, "u");
        assert_eq!(p.model, "m");
        assert_eq!(p.key, None);
        assert!(matches!(app.mode, Mode::Browse), "returns to browse");
    }

    #[test]
    fn edit_action_changes_only_the_edited_field() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "u".into(), "m".into(), Some("sk".into()))
            .unwrap();
        // Reference it from a group to prove the edit needs no unbinding.
        app.ed
            .set_group("g".into(), vec!["p1".into()], Strategy::Failover);
        on_key(&mut app, KeyCode::Char('e')); // edit selected (p1) → base_url prefilled
        on_key(&mut app, KeyCode::Enter); // keep base_url
        // model field is prefilled with "m"; replace it with "m2".
        on_key(&mut app, KeyCode::Backspace);
        type_str(&mut app, "m2");
        on_key(&mut app, KeyCode::Enter); // → key
        on_key(&mut app, KeyCode::Enter); // blank key → keep current, finalize
        let p = &app.ed.providers()[0];
        assert_eq!(p.model, "m2");
        assert_eq!(p.base_url, "u", "unchanged field preserved");
        assert_eq!(p.key.as_deref(), Some("sk"), "blank key keeps current");
        assert_eq!(
            app.ed.cfg.group("g").unwrap().members,
            vec!["p1".to_string()],
            "group binding intact"
        );
    }

    #[test]
    fn delete_requires_confirmation() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "u".into(), "m".into(), None)
            .unwrap();
        // A single `d` does not remove — it opens a confirmation naming p1.
        on_key(&mut app, KeyCode::Char('d'));
        assert_eq!(app.ed.providers().len(), 1, "single d must not delete");
        match &app.mode {
            Mode::ConfirmDelete { name } => assert_eq!(name, "p1"),
            _ => panic!("expected a confirmation prompt"),
        }
        // Cancelling keeps the provider.
        on_key(&mut app, KeyCode::Char('n'));
        assert_eq!(app.ed.providers().len(), 1);
        assert!(matches!(app.mode, Mode::Browse));
        // Confirming removes it.
        on_key(&mut app, KeyCode::Char('d'));
        on_key(&mut app, KeyCode::Char('y'));
        assert!(app.ed.providers().is_empty(), "confirmed delete removes p1");
    }

    #[test]
    fn group_remove_and_role_unset_keys() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "u".into(), "m".into(), None)
            .unwrap();
        app.ed
            .set_group("g".into(), vec!["p1".into()], Strategy::Failover);
        app.ed.set_role("main".into(), "g".into());
        // `G` removes a group, but a role still targets it → guard names the role.
        on_key(&mut app, KeyCode::Char('G'));
        type_str(&mut app, "g");
        on_key(&mut app, KeyCode::Enter);
        assert!(
            app.status.contains("main"),
            "guard names the referring role: {}",
            app.status
        );
        assert!(app.ed.cfg.group("g").is_some(), "group left in place");
        // `R` unsets the role, then `G` removes the now-unreferenced group.
        on_key(&mut app, KeyCode::Char('R'));
        type_str(&mut app, "main");
        on_key(&mut app, KeyCode::Enter);
        assert!(!app.ed.cfg.roles.contains_key("main"), "role unset");
        on_key(&mut app, KeyCode::Char('G'));
        type_str(&mut app, "g");
        on_key(&mut app, KeyCode::Enter);
        assert!(app.ed.cfg.group("g").is_none(), "group removed after unbind");
    }

    #[test]
    fn help_line_lists_new_keys() {
        let app = App::new(ProvidersConfig::default());
        for token in ["e edit", "G rm-group", "R unset-role"] {
            assert!(
                app.status.contains(token),
                "status line shows '{token}': {}",
                app.status
            );
        }
    }
}
