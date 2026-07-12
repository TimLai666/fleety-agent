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

/// Parse `p1/m1|p2/m2` into members. Test-only: the guided wizard builds a
/// single member directly; multi-member pools use the non-interactive
/// `config model` subcommand.
#[cfg(test)]
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

/// Fetch `{base_url}/models` synchronously from inside the TUI's blocking key
/// loop. The loop runs on the multi-threaded tokio runtime (`#[tokio::main]`),
/// so `block_in_place` + the current runtime handle drives the async fetch
/// without a nested runtime. Any error (including "no runtime" when called from
/// a plain thread, e.g. a test) is returned as a `String` the wizard turns into
/// manual entry.
fn fetch_provider_models(
    base: &str,
    key: Option<String>,
) -> std::result::Result<Vec<String>, String> {
    let handle =
        tokio::runtime::Handle::try_current().map_err(|_| "no async runtime".to_string())?;
    let base = base.to_string();
    tokio::task::block_in_place(move || {
        handle.block_on(crate::model_picker::fetch_models(&base, key.as_deref()))
    })
    .map_err(|e| e.to_string())
}

// ---- interactive screen ----

/// A single-line action collecting one input line. (Set-model is a guided
/// wizard, not a line action — see [`ModelWizard`].)
enum Action {
    UnsetModel,
}

enum Mode {
    Browse,
    Input {
        action: Action,
        prompt: &'static str,
        buffer: LineEditor,
    },
    /// Guided, step-by-step add-provider: pick the type from a menu, then fill
    /// each field with its own prompt (replaces the old one comma-separated line).
    AddProvider(AddWizard),
    /// Guided, two-level set-model: pick the role, pick a provider, then pick
    /// one of that provider's fetched models (or type an id when the fetch is
    /// unavailable). Replaces the old comma-separated `role, members, strategy`.
    SetModel(ModelWizard),
}

/// One step of the add-provider wizard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AddStep {
    PickType,
    Name,
    BaseUrl,
    Key,
}

/// The next step after `step` for a provider type, skipping fields the type does
/// not use (oauth types carry no base_url/key). `None` = the wizard is complete.
/// Pure — unit-testable.
fn add_next_step(step: AddStep, requires_base_url: bool, allows_key: bool) -> Option<AddStep> {
    match step {
        AddStep::PickType => Some(AddStep::Name),
        AddStep::Name => {
            if requires_base_url {
                Some(AddStep::BaseUrl)
            } else if allows_key {
                Some(AddStep::Key)
            } else {
                None
            }
        }
        AddStep::BaseUrl => allows_key.then_some(AddStep::Key),
        AddStep::Key => None,
    }
}

/// Accumulating state for the add-provider wizard.
struct AddWizard {
    step: AddStep,
    type_sel: usize,
    kind: String,
    name: String,
    base_url: String,
    key: String,
    buffer: LineEditor,
}

impl AddWizard {
    fn new() -> Self {
        Self {
            step: AddStep::PickType,
            type_sel: 0,
            kind: String::new(),
            name: String::new(),
            base_url: String::new(),
            key: String::new(),
            buffer: LineEditor::default(),
        }
    }

    fn selected_type(&self) -> Option<&'static pc::ProviderType> {
        pc::provider_types().get(self.type_sel)
    }

    /// Store the just-entered field text and advance; returns `true` when the
    /// wizard is complete (the caller then calls `add_provider`).
    fn advance_text(&mut self, text: String) -> bool {
        let text = text.trim().to_string();
        match self.step {
            AddStep::Name => self.name = text,
            AddStep::BaseUrl => self.base_url = text,
            AddStep::Key => self.key = text,
            AddStep::PickType => {}
        }
        let ty = self.selected_type();
        let (rb, ak) = ty
            .map(|t| (t.requires_base_url, t.allows_key))
            .unwrap_or((false, false));
        match add_next_step(self.step, rb, ak) {
            Some(next) => {
                self.step = next;
                self.buffer = LineEditor::default();
                false
            }
            None => true,
        }
    }
}

/// The model roles a guided set-model can target. Kept in sync with the two
/// roles the runtime resolves (`main`, `cheap`); the wizard offers exactly these
/// so the user picks instead of free-typing a role name.
const MODEL_ROLES: &[&str] = &["main", "cheap"];

/// One step of the set-model wizard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ModelStep {
    PickRole,
    PickProvider,
    /// Transient: the run loop performs the `/models` fetch, then calls
    /// [`ModelWizard::apply_fetch`] to advance to `PickModel`.
    Fetching,
    PickModel,
}

/// Accumulating state for the two-level set-model wizard. The provider names are
/// captured at construction (non-empty — the caller guards on that), so the
/// role→provider steps are pure navigation; only the provider→model transition
/// touches the network, and that runs in the loop (not in `on_key`), keeping key
/// handling unit-testable.
struct ModelWizard {
    step: ModelStep,
    role_sel: usize,
    role: String,
    prov_names: Vec<String>,
    prov_sel: usize,
    provider: String,
    /// Models fetched from the provider's `/models`. Empty when the fetch was
    /// skipped or failed — then `manual` is set and the user types the id.
    models: Vec<String>,
    model_sel: usize,
    manual: bool,
    /// Search box in list mode; the whole model id in manual mode.
    filter: LineEditor,
    /// One-line hint shown in the title (model count, or why it fell to manual).
    note: String,
}

impl ModelWizard {
    fn new(prov_names: Vec<String>) -> Self {
        Self {
            step: ModelStep::PickRole,
            role_sel: 0,
            role: String::new(),
            prov_names,
            prov_sel: 0,
            provider: String::new(),
            models: Vec::new(),
            model_sel: 0,
            manual: false,
            filter: LineEditor::default(),
            note: String::new(),
        }
    }

    /// The models matching the current filter box (list mode).
    fn filtered(&self) -> Vec<&String> {
        crate::model_picker::filter_models(&self.models, self.filter.text())
    }

    /// Consume the `/models` fetch outcome and advance to `PickModel`: a
    /// non-empty list becomes a searchable pick; an empty list or an error
    /// degrades to manual entry (the wizard never dead-ends on a bad fetch).
    /// Pure — unit-tested.
    fn apply_fetch(&mut self, result: std::result::Result<Vec<String>, String>) {
        match result {
            Ok(models) if !models.is_empty() => {
                self.note = format!("{} models · type to filter", models.len());
                self.models = models;
                self.manual = false;
            }
            Ok(_) => {
                self.models = Vec::new();
                self.manual = true;
                self.note = "no models returned · type the id".to_string();
            }
            Err(e) => {
                self.models = Vec::new();
                self.manual = true;
                self.note = format!("fetch failed ({e}) · type the id");
            }
        }
        self.step = ModelStep::PickModel;
        self.model_sel = 0;
        self.filter = LineEditor::default();
    }
}

struct App {
    ed: ProviderEditor,
    sel: usize,
    mode: Mode,
    status: String,
    save_now: bool,
    /// Set by the set-model wizard when it enters `Fetching`; the run loop does
    /// the blocking `/models` fetch (I/O belongs there, not in `on_key`) and
    /// feeds the result back via [`ModelWizard::apply_fetch`].
    fetch_models_now: bool,
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
            fetch_models_now: false,
            quit: false,
        }
    }

    /// Fetch the selected provider's model list for the in-progress set-model
    /// wizard. Reads the provider's `base_url`/`key` from the edited config; a
    /// provider without a `base_url` (e.g. oauth) or any fetch error is returned
    /// as `Err` so the wizard falls back to manual entry.
    fn fetch_selected_provider_models(&self) -> std::result::Result<Vec<String>, String> {
        let Mode::SetModel(w) = &self.mode else {
            return Err("not in set-model".to_string());
        };
        let p = self
            .ed
            .cfg
            .provider(&w.provider)
            .ok_or_else(|| format!("no provider '{}'", w.provider))?;
        let base = p
            .base_url
            .clone()
            .ok_or_else(|| "provider has no base_url".to_string())?;
        fetch_provider_models(&base, p.key.clone())
    }

    /// Apply the submitted input buffer for the active single-line action.
    fn submit(&mut self, action: &Action, buf: &str) {
        let parts: Vec<String> = buf.split(',').map(|s| s.trim().to_string()).collect();
        let res = match action {
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

/// Drive the guided add-provider wizard. Computes the outcome under a scoped
/// borrow of the wizard, then mutates `app` (so completing can touch `app.ed`).
fn on_key_add_wizard(app: &mut App, code: KeyCode) {
    enum WizAction {
        Stay,
        Cancel,
        Complete,
    }
    let action = {
        let Mode::AddProvider(wiz) = &mut app.mode else {
            return;
        };
        match wiz.step {
            AddStep::PickType => match code {
                KeyCode::Up => {
                    wiz.type_sel = wiz.type_sel.saturating_sub(1);
                    WizAction::Stay
                }
                KeyCode::Down => {
                    let max = pc::provider_types().len().saturating_sub(1);
                    wiz.type_sel = (wiz.type_sel + 1).min(max);
                    WizAction::Stay
                }
                KeyCode::Enter => {
                    wiz.kind = wiz
                        .selected_type()
                        .map(|t| t.name.to_string())
                        .unwrap_or_default();
                    wiz.step = AddStep::Name;
                    wiz.buffer = LineEditor::default();
                    WizAction::Stay
                }
                KeyCode::Esc => WizAction::Cancel,
                _ => WizAction::Stay,
            },
            AddStep::Name | AddStep::BaseUrl | AddStep::Key => match code {
                KeyCode::Char(c) => {
                    wiz.buffer.insert(c);
                    WizAction::Stay
                }
                KeyCode::Backspace => {
                    wiz.buffer.backspace();
                    WizAction::Stay
                }
                KeyCode::Left => {
                    wiz.buffer.left();
                    WizAction::Stay
                }
                KeyCode::Right => {
                    wiz.buffer.right();
                    WizAction::Stay
                }
                KeyCode::Esc => WizAction::Cancel,
                KeyCode::Enter => {
                    let text = wiz.buffer.take();
                    if wiz.advance_text(text) {
                        WizAction::Complete
                    } else {
                        WizAction::Stay
                    }
                }
                _ => WizAction::Stay,
            },
        }
    };
    match action {
        WizAction::Stay => {}
        WizAction::Cancel => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        WizAction::Complete => {
            if let Mode::AddProvider(w) = std::mem::replace(&mut app.mode, Mode::Browse) {
                let base_url = (!w.base_url.is_empty()).then_some(w.base_url);
                let key = (!w.key.is_empty()).then_some(w.key);
                app.status =
                    match app
                        .ed
                        .add_provider(w.name.clone(), w.kind.clone(), base_url, key)
                    {
                        Ok(()) => format!("added provider '{}'", w.name),
                        Err(e) => format!("error: {e}"),
                    };
            }
        }
    }
}

/// Drive the guided two-level set-model wizard. Like the add wizard, the
/// outcome is computed under a scoped borrow, then applied to `app` (Complete
/// touches `app.ed`; Fetch flags the run loop). Pure w.r.t. the network — the
/// `/models` fetch happens in the run loop, not here.
fn on_key_model_wizard(app: &mut App, code: KeyCode) {
    enum MdAction {
        Stay,
        Cancel,
        Fetch,
        Complete {
            role: String,
            provider: String,
            model: String,
        },
    }
    let action = {
        let Mode::SetModel(w) = &mut app.mode else {
            return;
        };
        match w.step {
            ModelStep::PickRole => match code {
                KeyCode::Up => {
                    w.role_sel = w.role_sel.saturating_sub(1);
                    MdAction::Stay
                }
                KeyCode::Down => {
                    w.role_sel = (w.role_sel + 1).min(MODEL_ROLES.len() - 1);
                    MdAction::Stay
                }
                KeyCode::Enter => {
                    w.role = MODEL_ROLES[w.role_sel].to_string();
                    w.step = ModelStep::PickProvider;
                    w.prov_sel = 0;
                    MdAction::Stay
                }
                KeyCode::Esc => MdAction::Cancel,
                _ => MdAction::Stay,
            },
            ModelStep::PickProvider => match code {
                KeyCode::Up => {
                    w.prov_sel = w.prov_sel.saturating_sub(1);
                    MdAction::Stay
                }
                KeyCode::Down => {
                    w.prov_sel = (w.prov_sel + 1).min(w.prov_names.len().saturating_sub(1));
                    MdAction::Stay
                }
                KeyCode::Enter => {
                    w.provider = w.prov_names.get(w.prov_sel).cloned().unwrap_or_default();
                    w.step = ModelStep::Fetching;
                    MdAction::Fetch
                }
                KeyCode::Esc => {
                    w.step = ModelStep::PickRole;
                    MdAction::Stay
                }
                _ => MdAction::Stay,
            },
            // The run loop owns this step; only allow bailing out.
            ModelStep::Fetching => match code {
                KeyCode::Esc => MdAction::Cancel,
                _ => MdAction::Stay,
            },
            ModelStep::PickModel => {
                // Manual entry: the filter box IS the model id.
                if w.manual {
                    match code {
                        KeyCode::Char(c) => {
                            w.filter.insert(c);
                            MdAction::Stay
                        }
                        KeyCode::Backspace => {
                            w.filter.backspace();
                            MdAction::Stay
                        }
                        KeyCode::Left => {
                            w.filter.left();
                            MdAction::Stay
                        }
                        KeyCode::Right => {
                            w.filter.right();
                            MdAction::Stay
                        }
                        KeyCode::Esc => {
                            w.step = ModelStep::PickProvider;
                            MdAction::Stay
                        }
                        KeyCode::Enter => {
                            let model = w.filter.text().trim().to_string();
                            if model.is_empty() {
                                MdAction::Stay
                            } else {
                                MdAction::Complete {
                                    role: w.role.clone(),
                                    provider: w.provider.clone(),
                                    model,
                                }
                            }
                        }
                        _ => MdAction::Stay,
                    }
                } else {
                    // Searchable list: type to filter, ↑↓ to move the highlight.
                    match code {
                        KeyCode::Up => {
                            w.model_sel = w.model_sel.saturating_sub(1);
                            MdAction::Stay
                        }
                        KeyCode::Down => {
                            let max = w.filtered().len().saturating_sub(1);
                            w.model_sel = (w.model_sel + 1).min(max);
                            MdAction::Stay
                        }
                        KeyCode::Char(c) => {
                            w.filter.insert(c);
                            w.model_sel = 0;
                            MdAction::Stay
                        }
                        KeyCode::Backspace => {
                            w.filter.backspace();
                            w.model_sel = 0;
                            MdAction::Stay
                        }
                        KeyCode::Esc => {
                            w.step = ModelStep::PickProvider;
                            MdAction::Stay
                        }
                        KeyCode::Enter => {
                            // Highlighted match wins; otherwise a non-empty
                            // filter is taken as a typed-in id (manual override).
                            let picked = w.filtered().get(w.model_sel).map(|m| (*m).clone());
                            let model =
                                picked.unwrap_or_else(|| w.filter.text().trim().to_string());
                            if model.is_empty() {
                                MdAction::Stay
                            } else {
                                MdAction::Complete {
                                    role: w.role.clone(),
                                    provider: w.provider.clone(),
                                    model,
                                }
                            }
                        }
                        _ => MdAction::Stay,
                    }
                }
            }
        }
    };
    match action {
        MdAction::Stay => {}
        MdAction::Cancel => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        MdAction::Fetch => {
            app.fetch_models_now = true;
        }
        MdAction::Complete {
            role,
            provider,
            model,
        } => {
            app.ed.set_model(
                role.clone(),
                vec![Member {
                    provider,
                    model: model.clone(),
                    stream: false,
                    modalities: None,
                    effort: None,
                }],
                Strategy::Single,
            );
            app.mode = Mode::Browse;
            app.status = format!("set {role} = {model} (single)");
        }
    }
}

fn on_key(app: &mut App, code: KeyCode) {
    if matches!(app.mode, Mode::AddProvider(_)) {
        on_key_add_wizard(app, code);
        return;
    }
    if matches!(app.mode, Mode::SetModel(_)) {
        on_key_model_wizard(app, code);
        return;
    }
    match &mut app.mode {
        Mode::AddProvider(_) | Mode::SetModel(_) => {} // handled above
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
                app.mode = Mode::AddProvider(AddWizard::new());
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
                let names = app.ed.provider_names();
                if names.is_empty() {
                    app.status = "add a provider first".to_string();
                } else {
                    app.mode = Mode::SetModel(ModelWizard::new(names));
                }
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
        Mode::AddProvider(wiz) => match wiz.step {
            AddStep::PickType => {
                let sel = pc::provider_types()
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        if i == wiz.type_sel {
                            format!("▶{}", t.name)
                        } else {
                            format!(" {}", t.name)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("   ");
                f.render_widget(
                    Paragraph::new(format!("type: {sel}")).block(
                        Block::bordered()
                            .title("add provider — ↑↓ pick type · Enter next · Esc cancel"),
                    ),
                    chunks[1],
                );
            }
            AddStep::Name | AddStep::BaseUrl | AddStep::Key => {
                let (label, masked) = match wiz.step {
                    AddStep::BaseUrl => ("base_url", false),
                    AddStep::Key => ("api key", true),
                    _ => ("name", false),
                };
                let max_w = chunks[1].width.saturating_sub(2) as usize;
                let (view, x) = wiz.buffer.display_window(max_w.saturating_sub(2));
                let shown = if masked {
                    "*".repeat(view.chars().count())
                } else {
                    view.to_string()
                };
                f.render_widget(
                    Paragraph::new(format!("> {shown}")).block(Block::bordered().title(format!(
                        "add {} provider — {label} · Enter next · Esc cancel",
                        wiz.kind
                    ))),
                    chunks[1],
                );
                if !masked {
                    f.set_cursor_position((
                        chunks[1].x + 1 + (2 + x as usize).min(max_w) as u16,
                        chunks[1].y + 1,
                    ));
                }
            }
        },
        Mode::SetModel(w) => match w.step {
            ModelStep::PickRole => {
                let sel = MODEL_ROLES
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        if i == w.role_sel {
                            format!("▶{r}")
                        } else {
                            format!(" {r}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("   ");
                f.render_widget(
                    Paragraph::new(format!("role: {sel}")).block(
                        Block::bordered()
                            .title("set model — ↑↓ pick role · Enter next · Esc cancel"),
                    ),
                    chunks[1],
                );
            }
            ModelStep::PickProvider => {
                let n = w.prov_names.len();
                let name = w
                    .prov_names
                    .get(w.prov_sel)
                    .map(String::as_str)
                    .unwrap_or("");
                f.render_widget(
                    Paragraph::new(format!("provider [{}/{n}]: {name}", w.prov_sel + 1)).block(
                        Block::bordered().title(format!(
                            "set model [{}] — ↑↓ provider · Enter fetch models · Esc back",
                            w.role
                        )),
                    ),
                    chunks[1],
                );
            }
            ModelStep::Fetching => {
                f.render_widget(
                    Paragraph::new(format!("fetching {}/models …", w.provider))
                        .block(Block::bordered().title(format!("set model [{}]", w.role))),
                    chunks[1],
                );
            }
            ModelStep::PickModel => {
                if w.manual {
                    let max_w = chunks[1].width.saturating_sub(2) as usize;
                    let (view, x) = w.filter.display_window(max_w.saturating_sub(2));
                    f.render_widget(
                        Paragraph::new(format!("> {view}")).block(Block::bordered().title(
                            format!(
                                "set model [{}] {} — {} · Enter save · Esc back",
                                w.role, w.provider, w.note
                            ),
                        )),
                        chunks[1],
                    );
                    f.set_cursor_position((
                        chunks[1].x + 1 + (2 + x as usize).min(max_w) as u16,
                        chunks[1].y + 1,
                    ));
                } else {
                    let filt = w.filtered();
                    let n = filt.len();
                    let highlighted = filt
                        .get(w.model_sel)
                        .map(|m| m.as_str())
                        .unwrap_or("(no match — Enter uses the filter as id)");
                    let idx = if n == 0 { 0 } else { w.model_sel + 1 };
                    let ftext = w.filter.text();
                    let filt_hint = if ftext.is_empty() {
                        String::new()
                    } else {
                        format!("   filter: {ftext}")
                    };
                    f.render_widget(
                        Paragraph::new(format!("[{idx}/{n}] {highlighted}{filt_hint}")).block(
                            Block::bordered().title(format!(
                                "set model [{}] {} — ↑↓ pick · type filter · Enter save · Esc back",
                                w.role, w.provider
                            )),
                        ),
                        chunks[1],
                    );
                }
            }
        },
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
                    if app.fetch_models_now {
                        // The set-model wizard asked to fetch the provider's
                        // models: do the blocking `/models` request here (I/O
                        // belongs in the loop) and feed the result back.
                        app.fetch_models_now = false;
                        let result = app.fetch_selected_provider_models();
                        if let Mode::SetModel(w) = &mut app.mode {
                            w.apply_fetch(result);
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
    fn add_next_step_skips_unused_fields() {
        // api (base_url + key): Name → BaseUrl → Key → done.
        assert_eq!(
            add_next_step(AddStep::Name, true, true),
            Some(AddStep::BaseUrl)
        );
        assert_eq!(
            add_next_step(AddStep::BaseUrl, true, true),
            Some(AddStep::Key)
        );
        assert_eq!(add_next_step(AddStep::Key, true, true), None);
        // oauth (no base_url, no key): Name → done.
        assert_eq!(add_next_step(AddStep::Name, false, false), None);
        // base_url only: Name → BaseUrl → done.
        assert_eq!(add_next_step(AddStep::BaseUrl, true, false), None);
    }

    #[test]
    fn add_provider_via_wizard() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // wizard: PickType (api at index 0)
        on_key(&mut app, KeyCode::Enter); // choose api → Name step
        type_str(&mut app, "openai1");
        on_key(&mut app, KeyCode::Enter); // → base_url
        type_str(&mut app, "https://u/v1");
        on_key(&mut app, KeyCode::Enter); // → api key
        type_str(&mut app, "sk");
        on_key(&mut app, KeyCode::Enter); // done → add
        assert_eq!(app.ed.provider_names(), vec!["openai1".to_string()]);
        let p = app.ed.cfg.provider("openai1").unwrap();
        assert_eq!(p.kind, "api");
        assert_eq!(p.base_url.as_deref(), Some("https://u/v1"));
        assert_eq!(p.key.as_deref(), Some("sk"));
    }

    #[test]
    fn apply_fetch_picks_list_or_manual() {
        let mut w = ModelWizard::new(vec!["p1".to_string()]);
        // A non-empty list → searchable pick (not manual), models retained.
        w.apply_fetch(Ok(vec!["gpt-4o".into(), "gpt-4o-mini".into()]));
        assert!(!w.manual);
        assert_eq!(w.step, ModelStep::PickModel);
        assert_eq!(w.filtered().len(), 2);
        // An empty list → manual entry.
        let mut w = ModelWizard::new(vec!["p1".to_string()]);
        w.apply_fetch(Ok(vec![]));
        assert!(w.manual);
        // An error → manual entry (never dead-ends), note carries the reason.
        let mut w = ModelWizard::new(vec!["p1".to_string()]);
        w.apply_fetch(Err("boom".into()));
        assert!(w.manual);
        assert!(w.note.contains("boom"));
    }

    /// 'm' with no providers is a no-op with a helpful hint (guard, not a crash).
    #[test]
    fn set_model_needs_a_provider_first() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('m'));
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.status.contains("add a provider"));
    }

    #[test]
    fn set_model_via_wizard_manual_when_fetch_fails() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        on_key(&mut app, KeyCode::Char('m')); // PickRole (main at 0)
        on_key(&mut app, KeyCode::Enter); // role=main → PickProvider
        on_key(&mut app, KeyCode::Enter); // provider=p1 → Fetching (flags fetch)
        assert!(app.fetch_models_now);
        // Simulate the run loop's fetch failing → manual entry.
        if let Mode::SetModel(w) = &mut app.mode {
            w.apply_fetch(Err("no network".into()));
        }
        type_str(&mut app, "gpt-4o");
        on_key(&mut app, KeyCode::Enter); // save
        let pool = app.ed.cfg.model("main").unwrap();
        assert_eq!(pool.strategy, Strategy::Single);
        assert_eq!(pool.members.len(), 1);
        assert_eq!(pool.members[0].provider, "p1");
        assert_eq!(pool.members[0].model, "gpt-4o");
    }

    #[test]
    fn set_model_via_wizard_picks_from_fetched_list() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        on_key(&mut app, KeyCode::Char('m'));
        on_key(&mut app, KeyCode::Enter); // role=main
        on_key(&mut app, KeyCode::Enter); // provider=p1
        if let Mode::SetModel(w) = &mut app.mode {
            w.apply_fetch(Ok(vec!["gpt-4o".into(), "gpt-4o-mini".into()]));
        }
        type_str(&mut app, "mini"); // filter down to the one match
        on_key(&mut app, KeyCode::Enter); // pick the highlighted match
        let pool = app.ed.cfg.model("main").unwrap();
        assert_eq!(pool.members.len(), 1);
        assert_eq!(pool.members[0].model, "gpt-4o-mini");
    }

    #[test]
    fn wizard_shows_the_typed_buffer() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // PickType
        on_key(&mut app, KeyCode::Enter); // → Name step (text input)
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
        assert!(
            content.contains("add api provider"),
            "wizard prompt visible in title"
        );
    }
}
