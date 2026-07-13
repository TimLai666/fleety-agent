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

use std::collections::BTreeMap;
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

    /// Upsert a provider: replace an existing entry (edit) or add a new one. No
    /// duplicate guard (unlike [`add_provider`](Self::add_provider)) — this is the
    /// edit path, where the name already exists. Field-shape rules (api needs
    /// base_url; oauth carries none) are enforced at [`save`].
    pub fn set_provider(
        &mut self,
        name: String,
        kind: String,
        base_url: Option<String>,
        key: Option<String>,
    ) {
        self.cfg.providers.insert(
            name,
            Provider {
                kind,
                base_url,
                key,
            },
        );
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelFetchTarget {
    Api {
        base_url: String,
        key: Option<String>,
    },
    Server {
        provider: String,
    },
}

/// Select the discovery authority before touching a provider endpoint. OAuth
/// providers have no `base_url`; remote editors use the server authority.
fn model_fetch_target(
    provider_name: &str,
    provider: &Provider,
    server_discovery: bool,
) -> std::result::Result<ModelFetchTarget, String> {
    if provider.kind == "oauth:codex" {
        return if server_discovery {
            Ok(ModelFetchTarget::Server {
                provider: provider_name.to_string(),
            })
        } else {
            Err("OAuth model discovery requires a connected server".to_string())
        };
    }
    let base_url = provider
        .base_url
        .clone()
        .ok_or_else(|| "provider has no base_url".to_string())?;
    Ok(ModelFetchTarget::Api {
        base_url,
        key: provider.key.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAuthState {
    SignedIn,
    NotSignedIn,
    Unavailable,
}

pub type ProviderAuthStates = BTreeMap<String, ProviderAuthState>;

fn auth_state_label(state: ProviderAuthState) -> &'static str {
    match state {
        ProviderAuthState::SignedIn => "signed in",
        ProviderAuthState::NotSignedIn => "not signed in",
        ProviderAuthState::Unavailable => "unavailable",
    }
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
    /// Edit an existing **api** provider: change its base_url (prefilled) and api
    /// key (blank keeps the current one). The name and type stay fixed.
    EditProvider(EditWizard),
    /// The OAuth action menu for an existing **oauth:codex** provider: sign in,
    /// sign out, or switch account. The chosen action is carried out after the
    /// editor exits (the browser flow can't run under the full-screen UI).
    OauthActions(OauthMenu),
}

/// The two editable fields of an api provider, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditStep {
    BaseUrl,
    Key,
}

/// Accumulating state for editing an existing api provider. `base_url` starts
/// prefilled; `key` starts empty and, left blank, keeps the current key.
struct EditWizard {
    name: String,
    kind: String,
    /// The provider's current key, preserved when the user leaves the key blank.
    existing_key: Option<String>,
    step: EditStep,
    base_url: String,
    buffer: LineEditor,
}

impl EditWizard {
    fn new(name: String, kind: String, base_url: Option<String>, key: Option<String>) -> Self {
        let mut buffer = LineEditor::default();
        buffer.set_text(base_url.clone().unwrap_or_default());
        Self {
            name,
            kind,
            existing_key: key,
            step: EditStep::BaseUrl,
            base_url: base_url.unwrap_or_default(),
            buffer,
        }
    }

    /// Store the current field and advance. Returns `Some((base_url, key))` when
    /// the edit is complete (the caller upserts it). A blank key keeps the
    /// existing one; a non-blank key replaces it.
    fn advance(&mut self, text: String) -> Option<(Option<String>, Option<String>)> {
        match self.step {
            EditStep::BaseUrl => {
                self.base_url = text.trim().to_string();
                self.step = EditStep::Key;
                self.buffer = LineEditor::default();
                None
            }
            EditStep::Key => {
                let entered = text.trim().to_string();
                let key = if entered.is_empty() {
                    self.existing_key.clone()
                } else {
                    Some(entered)
                };
                let base_url = (!self.base_url.is_empty()).then(|| self.base_url.clone());
                Some((base_url, key))
            }
        }
    }
}

/// The OAuth actions offered for an `oauth:codex` provider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuthAction {
    Login,
    Logout,
    Switch,
}

const AUTH_ACTIONS: &[(&str, AuthAction)] = &[
    ("Sign in", AuthAction::Login),
    ("Sign out", AuthAction::Logout),
    ("Switch account (sign out, then in)", AuthAction::Switch),
];

/// The OAuth action menu for one `oauth:codex` provider.
struct OauthMenu {
    provider: String,
    sel: usize,
}

/// What the editor asks the caller to do after it exits: run an OAuth action for
/// a provider (the browser flow needs the plain terminal, so it can't run under
/// the full-screen UI).
pub struct AuthRequest {
    pub action: AuthAction,
    pub provider: String,
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
    auth_states: ProviderAuthStates,
    save_now: bool,
    /// Set by the set-model wizard when it enters `Fetching`; the run loop does
    /// the blocking `/models` fetch (I/O belongs there, not in `on_key`) and
    /// feeds the result back via [`ModelWizard::apply_fetch`].
    fetch_models_now: bool,
    /// Set when the user picks an OAuth action for a provider: the editor saves
    /// and quits, and the caller runs the action (browser flow) after the
    /// full-screen UI is torn down.
    auth_request: Option<AuthRequest>,
    quit: bool,
}

impl App {
    #[cfg(test)]
    fn new(cfg: ProvidersConfig) -> Self {
        Self::with_auth_states(cfg, ProviderAuthStates::new())
    }

    fn with_auth_states(cfg: ProvidersConfig, auth_states: ProviderAuthStates) -> Self {
        Self {
            ed: ProviderEditor::new(cfg),
            sel: 0,
            mode: Mode::Browse,
            // The key hints live on their own persistent footer line (see
            // `hints_for`); the status line starts empty and shows action results.
            status: "editing providers.toml".to_string(),
            auth_states,
            save_now: false,
            fetch_models_now: false,
            auth_request: None,
            quit: false,
        }
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
                let is_oauth = w.kind.eq_ignore_ascii_case("oauth:codex");
                match app
                    .ed
                    .add_provider(w.name.clone(), w.kind.clone(), base_url, key)
                {
                    Ok(()) => {
                        // An oauth:codex provider is useless until it is signed in,
                        // so go straight into login: save the new provider (to the
                        // server, via the caller) then run the sign-in flow —
                        // exactly what the OAuth-action menu does.
                        if is_oauth {
                            app.auth_request = Some(AuthRequest {
                                action: AuthAction::Login,
                                provider: w.name.clone(),
                            });
                            app.save_now = true;
                            app.quit = true;
                            app.status = format!("added '{}' — signing in…", w.name);
                        } else {
                            app.status = format!("added provider '{}'", w.name);
                        }
                    }
                    Err(e) => app.status = format!("error: {e}"),
                }
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

/// Drive the edit-existing-api-provider wizard (base_url then api key). Completing
/// upserts the provider via [`ProviderEditor::set_provider`].
fn on_key_edit_wizard(app: &mut App, code: KeyCode) {
    enum EdAction {
        Stay,
        Cancel,
        Complete {
            name: String,
            kind: String,
            base_url: Option<String>,
            key: Option<String>,
        },
    }
    let action = {
        let Mode::EditProvider(w) = &mut app.mode else {
            return;
        };
        match code {
            KeyCode::Char(c) => {
                w.buffer.insert(c);
                EdAction::Stay
            }
            KeyCode::Backspace => {
                w.buffer.backspace();
                EdAction::Stay
            }
            KeyCode::Left => {
                w.buffer.left();
                EdAction::Stay
            }
            KeyCode::Right => {
                w.buffer.right();
                EdAction::Stay
            }
            KeyCode::Esc => EdAction::Cancel,
            KeyCode::Enter => {
                let text = w.buffer.take();
                match w.advance(text) {
                    Some((base_url, key)) => EdAction::Complete {
                        name: w.name.clone(),
                        kind: w.kind.clone(),
                        base_url,
                        key,
                    },
                    None => EdAction::Stay,
                }
            }
            _ => EdAction::Stay,
        }
    };
    match action {
        EdAction::Stay => {}
        EdAction::Cancel => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        EdAction::Complete {
            name,
            kind,
            base_url,
            key,
        } => {
            app.ed.set_provider(name.clone(), kind, base_url, key);
            app.mode = Mode::Browse;
            app.status = format!("edited provider '{name}' (s to save)");
        }
    }
}

/// Drive the OAuth action menu for an `oauth:codex` provider. Enter records the
/// chosen action, saves, and quits so the caller runs it after the full-screen UI
/// is torn down (the browser flow can't run under ratatui).
fn on_key_oauth_menu(app: &mut App, code: KeyCode) {
    let Mode::OauthActions(m) = &mut app.mode else {
        return;
    };
    match code {
        KeyCode::Up => m.sel = m.sel.saturating_sub(1),
        KeyCode::Down => m.sel = (m.sel + 1).min(AUTH_ACTIONS.len() - 1),
        KeyCode::Esc => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        KeyCode::Enter => {
            let action = AUTH_ACTIONS[m.sel].1;
            let provider = m.provider.clone();
            // Save first (so the provider entry exists on the server the login
            // pushes to), then leave the editor to run the action.
            app.auth_request = Some(AuthRequest { action, provider });
            app.save_now = true;
            app.quit = true;
        }
        _ => {}
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
    if matches!(app.mode, Mode::EditProvider(_)) {
        on_key_edit_wizard(app, code);
        return;
    }
    if matches!(app.mode, Mode::OauthActions(_)) {
        on_key_oauth_menu(app, code);
        return;
    }
    match &mut app.mode {
        Mode::AddProvider(_)
        | Mode::SetModel(_)
        | Mode::EditProvider(_)
        | Mode::OauthActions(_) => {} // handled above
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
            KeyCode::Char('e') => match app.ed.provider_names().get(app.sel).cloned() {
                Some(name) => match app.ed.cfg.provider(&name) {
                    // oauth:codex → the sign-in/out/switch menu; api → field edit.
                    Some(p) if p.kind.eq_ignore_ascii_case("oauth:codex") => {
                        app.mode = Mode::OauthActions(OauthMenu {
                            provider: name,
                            sel: 0,
                        });
                    }
                    Some(p) => {
                        app.mode = Mode::EditProvider(EditWizard::new(
                            name,
                            p.kind.clone(),
                            p.base_url.clone(),
                            p.key.clone(),
                        ));
                    }
                    None => app.status = "nothing selected".to_string(),
                },
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
        .constraints([Constraint::Min(3), Constraint::Length(4)])
        .split(f.area());

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("Providers:"));
    for (i, (name, p)) in app.ed.cfg.providers.iter().enumerate() {
        let marker = if i == app.sel { "▶" } else { " " };
        let endpoint = p.base_url.as_deref().unwrap_or("(oauth login)");
        let auth = if p.kind == "oauth:codex" {
            format!(
                " auth={}",
                auth_state_label(
                    app.auth_states
                        .get(name)
                        .copied()
                        .unwrap_or(ProviderAuthState::Unavailable)
                )
            )
        } else {
            String::new()
        };
        lines.push(Line::from(format!(
            "{marker} {:<14} [{}] {} key={}{}",
            name,
            p.kind,
            endpoint,
            masked_key(&p.key),
            auth,
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

    // The footer is two lines inside one box: line 1 is the mode's content
    // (status / input / picker), line 2 is the persistent key hints. Because the
    // hints live on their own line, an action's status output (e.g. "added
    // provider 'X'") can never overwrite them.
    let max_w = chunks[1].width.saturating_sub(2) as usize;
    let cursor_col = |x: u16| -> u16 { (2 + x as usize).min(max_w) as u16 };
    let (title, body, cursor_x): (String, String, Option<u16>) = match &app.mode {
        Mode::Browse => ("providers.toml".to_string(), app.status.clone(), None),
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
                ("add provider".to_string(), format!("type: {sel}"), None)
            }
            AddStep::Name | AddStep::BaseUrl | AddStep::Key => {
                let (label, masked) = match wiz.step {
                    AddStep::BaseUrl => ("base_url", false),
                    AddStep::Key => ("api key", true),
                    _ => ("name", false),
                };
                let (view, x) = wiz.buffer.display_window(max_w.saturating_sub(2));
                let shown = if masked {
                    "*".repeat(view.chars().count())
                } else {
                    view.to_string()
                };
                (
                    format!("add {} provider — {label}", wiz.kind),
                    format!("> {shown}"),
                    (!masked).then(|| cursor_col(x)),
                )
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
                ("set model".to_string(), format!("role: {sel}"), None)
            }
            ModelStep::PickProvider => {
                let n = w.prov_names.len();
                let name = w
                    .prov_names
                    .get(w.prov_sel)
                    .map(String::as_str)
                    .unwrap_or("");
                (
                    format!("set model [{}]", w.role),
                    format!("provider [{}/{n}]: {name}", w.prov_sel + 1),
                    None,
                )
            }
            ModelStep::Fetching => (
                format!("set model [{}]", w.role),
                format!("fetching {}/models …", w.provider),
                None,
            ),
            ModelStep::PickModel => {
                if w.manual {
                    let (view, x) = w.filter.display_window(max_w.saturating_sub(2));
                    (
                        format!("set model [{}] {} — {}", w.role, w.provider, w.note),
                        format!("> {view}"),
                        Some(cursor_col(x)),
                    )
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
                    (
                        format!("set model [{}] {}", w.role, w.provider),
                        format!("[{idx}/{n}] {highlighted}{filt_hint}"),
                        None,
                    )
                }
            }
        },
        Mode::EditProvider(w) => {
            let (label, masked) = match w.step {
                EditStep::BaseUrl => ("base_url", false),
                EditStep::Key => ("api key (blank keeps current)", true),
            };
            let (view, x) = w.buffer.display_window(max_w.saturating_sub(2));
            let shown = if masked {
                "*".repeat(view.chars().count())
            } else {
                view.to_string()
            };
            (
                format!("edit {} provider '{}' — {label}", w.kind, w.name),
                format!("> {shown}"),
                (!masked).then(|| cursor_col(x)),
            )
        }
        Mode::OauthActions(m) => {
            let sel = AUTH_ACTIONS
                .iter()
                .enumerate()
                .map(|(i, (label, _))| {
                    if i == m.sel {
                        format!("▶{label}")
                    } else {
                        format!(" {label}")
                    }
                })
                .collect::<Vec<_>>()
                .join("   ");
            (format!("Codex sign-in — '{}'", m.provider), sel, None)
        }
        Mode::Input { prompt, buffer, .. } => {
            let (view, x) = buffer.display_window(max_w.saturating_sub(2));
            (prompt.to_string(), format!("> {view}"), Some(cursor_col(x)))
        }
    };
    f.render_widget(
        Paragraph::new(vec![Line::from(body), Line::from(hints_for(&app.mode))])
            .block(Block::bordered().title(title)),
        chunks[1],
    );
    if let Some(cx) = cursor_x {
        f.set_cursor_position((chunks[1].x + 1 + cx, chunks[1].y + 1));
    }
}

/// The persistent key-hint line for the current screen. Rendered on its own
/// footer line so an action's status output never hides it (spec: the key hints
/// stay visible). Pure — a non-empty hint exists for every screen.
fn hints_for(mode: &Mode) -> &'static str {
    match mode {
        Mode::Browse => "a: add · e: edit · d: del · m: set-model · u: unset · s: save · q: quit",
        Mode::AddProvider(w) => match w.step {
            AddStep::PickType => "↑↓: pick type · Enter: next · Esc: cancel",
            _ => "type the value · Enter: next · Esc: cancel",
        },
        Mode::SetModel(w) => match w.step {
            ModelStep::PickRole => "↑↓: pick role · Enter: next · Esc: cancel",
            ModelStep::PickProvider => "↑↓: provider · Enter: fetch models · Esc: back",
            ModelStep::Fetching => "fetching the provider's models…",
            ModelStep::PickModel => "↑↓: pick · type: filter · Enter: set · Esc: back",
        },
        Mode::EditProvider(_) => "type the value · Enter: next · Esc: cancel",
        Mode::OauthActions(_) => "↑↓: action · Enter: run · Esc: back",
        Mode::Input { .. } => "Enter: save · Esc: cancel",
    }
}

/// Why the editor exited (beyond a normal quit): a concurrent-edit conflict the
/// caller should reload from, and/or an OAuth action to run once the full-screen
/// UI is torn down (then reopen the editor).
pub struct EditorOutcome {
    pub conflict: Option<String>,
    pub auth_request: Option<AuthRequest>,
}

/// Open the interactive providers editor over `path` (this host's own file).
/// Returns an OAuth action the caller must run after the terminal is restored
/// (then reopen), or `None` on a normal quit. A local file write never conflicts.
pub fn run(path: &Path) -> Result<Option<AuthRequest>> {
    let cfg = pc::load_or_default(path)?;
    run_with_saver(cfg, |edited| {
        pc::write_providers(path, edited).map(|()| SaveOutcome::Saved)
    })
    .map(|outcome| outcome.auth_request)
}

/// Open the interactive providers editor over an in-memory configuration; every
/// save goes through `save` (local file write or remote apply — the editor does
/// not care). Returns an [`EditorOutcome`]: a conflict message when a save hit a
/// concurrent edit (the caller reloads and reopens), and/or a requested OAuth
/// action (the caller runs it after the UI is torn down, then reopens).
pub fn run_with_saver(
    cfg: ProvidersConfig,
    save: impl FnMut(&ProvidersConfig) -> Result<SaveOutcome>,
) -> Result<EditorOutcome> {
    run_with_saver_and_fetcher(
        cfg,
        save,
        |provider_name, provider| match model_fetch_target(provider_name, provider, false)? {
            ModelFetchTarget::Api { base_url, key } => fetch_provider_models(&base_url, key),
            ModelFetchTarget::Server { .. } => {
                Err("OAuth model discovery requires a connected server".to_string())
            }
        },
        ProviderAuthStates::new(),
    )
}

/// Open the editor with caller-owned model discovery and credential-status
/// providers. The remote config editor uses this to keep both operations on its
/// existing authenticated connection; the local wrapper above retains the API
/// endpoint behavior.
pub fn run_with_saver_and_fetcher(
    cfg: ProvidersConfig,
    mut save: impl FnMut(&ProvidersConfig) -> Result<SaveOutcome>,
    mut fetch_models: impl FnMut(&str, &Provider) -> std::result::Result<Vec<String>, String>,
    auth_states: ProviderAuthStates,
) -> Result<EditorOutcome> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut app = App::with_auth_states(cfg, auth_states);
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
                        // models. The caller supplies the authority, so remote
                        // OAuth discovery never needs a local base_url/token.
                        app.fetch_models_now = false;
                        let result = match &app.mode {
                            Mode::SetModel(w) => app
                                .ed
                                .cfg
                                .provider(&w.provider)
                                .ok_or_else(|| format!("no provider '{}'", w.provider))
                                .and_then(|provider| fetch_models(&w.provider, provider)),
                            _ => Err("not in set-model".to_string()),
                        };
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
    let auth_request = app.auth_request.take();
    result.map(|()| EditorOutcome {
        conflict,
        auth_request,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_model_target_uses_remote_discovery_without_base_url() {
        let provider = pc::Provider {
            kind: "oauth:codex".into(),
            base_url: None,
            key: None,
        };
        assert_eq!(
            model_fetch_target("tingzhen-codex", &provider, true),
            Ok(ModelFetchTarget::Server {
                provider: "tingzhen-codex".into()
            })
        );
    }

    #[test]
    fn oauth_provider_rows_render_signed_in_signed_out_and_unavailable_states() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "signed".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        cfg.providers.insert(
            "signed-out".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        cfg.providers.insert(
            "unknown".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        let mut states = ProviderAuthStates::new();
        states.insert("signed".into(), ProviderAuthState::SignedIn);
        states.insert("signed-out".into(), ProviderAuthState::NotSignedIn);
        let app = App::with_auth_states(cfg, states);
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("signed in"));
        assert!(content.contains("not signed in"));
        assert!(content.contains("auth=unavailable"));
    }

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
    fn set_provider_upserts_existing_fields() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider(
            "p1".into(),
            "api".into(),
            Some("https://old/v1".into()),
            Some("k1".into()),
        )
        .unwrap();
        // Upsert replaces the fields without the add-time duplicate guard.
        ed.set_provider(
            "p1".into(),
            "api".into(),
            Some("https://new/v1".into()),
            Some("k2".into()),
        );
        let p = ed.cfg.provider("p1").unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://new/v1"));
        assert_eq!(p.key.as_deref(), Some("k2"));
        assert_eq!(ed.provider_names(), vec!["p1".to_string()]);
    }

    #[test]
    fn edit_wizard_updates_base_url_and_keeps_blank_key() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider(
                "p1".into(),
                "api".into(),
                Some("https://old/v1".into()),
                Some("orig".into()),
            )
            .unwrap();
        on_key(&mut app, KeyCode::Char('e')); // api → EditProvider, base_url prefilled
        assert!(matches!(app.mode, Mode::EditProvider(_)));
        // Replace the base_url, then leave the key blank to keep the current one.
        if let Mode::EditProvider(w) = &mut app.mode {
            w.buffer = LineEditor::default();
        }
        type_str(&mut app, "https://new/v1");
        on_key(&mut app, KeyCode::Enter); // → key step
        on_key(&mut app, KeyCode::Enter); // blank key → complete
        let p = app.ed.cfg.provider("p1").unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://new/v1"));
        assert_eq!(
            p.key.as_deref(),
            Some("orig"),
            "blank key keeps the current one"
        );
    }

    #[test]
    fn adding_an_oauth_provider_goes_straight_to_login() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // add wizard: PickType (api at 0)
        on_key(&mut app, KeyCode::Down); // move to oauth:codex (index 1)
        on_key(&mut app, KeyCode::Enter); // pick oauth:codex → Name step
        type_str(&mut app, "tingzhen-codex");
        on_key(&mut app, KeyCode::Enter); // oauth has no base_url/key → Complete
                                          // The provider is added AND a sign-in is requested (save + quit), so the
                                          // caller runs login right away instead of leaving a signed-out shell.
        assert!(app.ed.cfg.provider("tingzhen-codex").is_some());
        let req = app
            .auth_request
            .as_ref()
            .expect("login request on oauth add");
        assert_eq!(req.action, AuthAction::Login);
        assert_eq!(req.provider, "tingzhen-codex");
        assert!(app.save_now, "saves the new provider before signing in");
        assert!(app.quit, "leaves the editor to run the browser flow");
    }

    #[test]
    fn oauth_provider_edit_opens_menu_and_records_request() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("codex1".into(), "oauth:codex".into(), None, None)
            .unwrap();
        on_key(&mut app, KeyCode::Char('e')); // oauth:codex → OauthActions menu
        assert!(matches!(app.mode, Mode::OauthActions(_)));
        on_key(&mut app, KeyCode::Down); // Sign in → Sign out
        on_key(&mut app, KeyCode::Down); // → Switch account
        on_key(&mut app, KeyCode::Enter);
        let req = app.auth_request.as_ref().expect("auth request recorded");
        assert_eq!(req.action, AuthAction::Switch);
        assert_eq!(req.provider, "codex1");
        assert!(app.save_now, "saves before running the action");
        assert!(app.quit, "leaves the editor to run the action");
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

    #[test]
    fn hints_for_is_nonempty_on_every_screen() {
        // Browse, both add-wizard step kinds, every set-model step, and the input
        // line each expose a non-empty key-hint line.
        assert!(!hints_for(&Mode::Browse).is_empty());
        assert!(!hints_for(&Mode::AddProvider(AddWizard::new())).is_empty()); // PickType
        let mut add = AddWizard::new();
        add.step = AddStep::Name;
        assert!(!hints_for(&Mode::AddProvider(add)).is_empty()); // text step
        for step in [
            ModelStep::PickRole,
            ModelStep::PickProvider,
            ModelStep::Fetching,
            ModelStep::PickModel,
        ] {
            let mut w = ModelWizard::new(vec!["p".to_string()]);
            w.step = step;
            assert!(!hints_for(&Mode::SetModel(w)).is_empty(), "{step:?}");
        }
        assert!(!hints_for(&Mode::Input {
            action: Action::UnsetModel,
            prompt: "x",
            buffer: LineEditor::default(),
        })
        .is_empty());
    }

    #[test]
    fn browse_hints_survive_a_status_message() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        // Add a provider through the wizard (sets status "added provider 'X'"),
        // then render: the status AND the key hints must both be visible.
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a'));
        on_key(&mut app, KeyCode::Enter); // api → name
        type_str(&mut app, "openai1");
        on_key(&mut app, KeyCode::Enter); // → base_url
        type_str(&mut app, "https://u/v1");
        on_key(&mut app, KeyCode::Enter); // → key
        type_str(&mut app, "sk");
        on_key(&mut app, KeyCode::Enter); // done → Browse, status set
        assert!(matches!(app.mode, Mode::Browse));
        let mut terminal = Terminal::new(TestBackend::new(90, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("added provider"), "status result visible");
        assert!(
            content.contains("a: add") && content.contains("q: quit"),
            "key hints stay visible alongside the status"
        );
    }
}
