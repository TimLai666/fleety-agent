use std::collections::BTreeMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveEntry {
    Bare,
    Chat,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryDecision {
    Help,
    Workspace(Route),
}

pub fn choose_entry(
    entry: InteractiveEntry,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> EntryDecision {
    match entry {
        InteractiveEntry::Bare if !(stdin_is_terminal && stdout_is_terminal) => EntryDecision::Help,
        InteractiveEntry::Bare | InteractiveEntry::Chat => EntryDecision::Workspace(Route::Chat),
        InteractiveEntry::Settings => {
            EntryDecision::Workspace(Route::Settings(SettingsPage::Connection))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Chat,
    Conversations,
    Settings(SettingsPage),
    ConnectionPicker,
    CommandPalette,
    Modal(Modal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Connection,
    Cli,
    Daemon,
    Server,
    ProvidersAndModels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Help {
        for_route: Box<Route>,
    },
    ConfirmExit,
    ResolveDirtyProfileSwitch {
        old_profile: String,
        new_profile: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, backoff_ms: u64 },
    AuthenticationRequired,
    Offline { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceContext {
    pub profile: Option<String>,
    pub endpoint: Option<String>,
    pub server_identity: Option<String>,
    pub server_version: Option<String>,
    pub daemon_device: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    Cli,
    Daemon,
    Server,
    ProvidersAndModels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerSnapshot {
    pub revision: String,
    pub values: BTreeMap<String, String>,
}

impl OwnerSnapshot {
    pub fn new(revision: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
            values: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceError {
    pub kind: String,
    pub message: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState<T> {
    Loading,
    Available(T),
    Dirty(T),
    Applying(T),
    Conflict(T, WorkspaceError),
    Failed(T, WorkspaceError),
    Unavailable(WorkspaceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticePersistence {
    Transient,
    UntilDismissed,
    UntilResolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: u64,
    pub severity: NoticeSeverity,
    pub summary: String,
    pub details: Option<String>,
    pub remediation: Option<String>,
    pub persistence: NoticePersistence,
}

impl Notice {
    pub fn error(summary: impl Into<String>) -> Self {
        Self {
            id: 0,
            severity: NoticeSeverity::Error,
            summary: summary.into(),
            details: None,
            remediation: None,
            persistence: NoticePersistence::UntilResolved,
        }
    }

    pub fn transient(summary: impl Into<String>) -> Self {
        Self {
            id: 0,
            severity: NoticeSeverity::Info,
            summary: summary.into(),
            details: None,
            remediation: None,
            persistence: NoticePersistence::Transient,
        }
    }

    pub fn details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    pub fn remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Navigate(Route),
    Back,
    OpenHelp,
    OpenCommandPalette,
    Connect,
    Connected,
    ConnectionLost {
        attempt: u32,
        backoff_ms: u64,
    },
    AuthenticationRequired,
    Offline(String),
    OwnerLoading(Owner),
    OwnerLoaded {
        owner: Owner,
        snapshot: OwnerSnapshot,
    },
    StageOwner {
        owner: Owner,
        snapshot: OwnerSnapshot,
    },
    ApplyOwner(Owner),
    OwnerApplied {
        owner: Owner,
        snapshot: OwnerSnapshot,
    },
    OwnerConflict {
        owner: Owner,
        error: WorkspaceError,
    },
    OwnerFailed {
        owner: Owner,
        error: WorkspaceError,
    },
    OwnerUnavailable {
        owner: Owner,
        error: WorkspaceError,
    },
    PushNotice(Notice),
    TransientStatus(String),
    DismissNotice(u64),
    ResolveNotices(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    ConnectCurrentProfile,
    ApplyOwner(Owner),
    CancelTurn,
    RetryNotice(u64),
    RunDoctor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommand {
    Chat,
    Conversations,
    Settings,
    SwitchProfile,
    Reconnect,
    Doctor,
    DismissNotice,
}

impl PaletteCommand {
    fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Conversations => "Conversations",
            Self::Settings => "Settings",
            Self::SwitchProfile => "Switch profile",
            Self::Reconnect => "Reconnect",
            Self::Doctor => "Doctor",
            Self::DismissNotice => "Dismiss notice",
        }
    }
}

const PALETTE_COMMANDS: [PaletteCommand; 7] = [
    PaletteCommand::Chat,
    PaletteCommand::Conversations,
    PaletteCommand::Settings,
    PaletteCommand::SwitchProfile,
    PaletteCommand::Reconnect,
    PaletteCommand::Doctor,
    PaletteCommand::DismissNotice,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceState {
    pub route: Route,
    pub history: Vec<Route>,
    pub connection: ConnectionState,
    pub context: WorkspaceContext,
    pub owners: BTreeMap<Owner, OwnerState<OwnerSnapshot>>,
    pub notices: Vec<Notice>,
    pub palette_query: String,
    pub palette_selected: usize,
    next_notice_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTransportContext {
    pub profile: String,
    pub endpoint: String,
    pub server_identity: Option<String>,
    pub server_version: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// The only terminal-key consumer for one interactive workspace. The reader is
/// started lazily so state-only tests and non-interactive dispatch do not leave
/// blocked terminal threads behind. Moving this value between routes preserves
/// one ordered stream instead of creating competing readers.
#[derive(Debug)]
struct StampedKey {
    epoch: u64,
    key: KeyEvent,
}

enum ReaderControl {
    Handoff {
        epoch: u64,
        acknowledged: tokio::sync::oneshot::Sender<()>,
    },
}

pub struct WorkspaceInput {
    receiver: Option<tokio::sync::mpsc::UnboundedReceiver<StampedKey>>,
    control: Option<std::sync::mpsc::Sender<ReaderControl>>,
    epoch: u64,
}

impl WorkspaceInput {
    pub fn terminal() -> Self {
        Self {
            receiver: None,
            control: None,
            epoch: 0,
        }
    }

    #[cfg(test)]
    fn from_receiver(receiver: tokio::sync::mpsc::UnboundedReceiver<KeyEvent>) -> Self {
        let (stamped_tx, stamped_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut receiver = receiver;
            while let Some(key) = receiver.recv().await {
                if stamped_tx.send(StampedKey { epoch: 0, key }).is_err() {
                    break;
                }
            }
        });
        Self {
            receiver: Some(stamped_rx),
            control: None,
            epoch: 0,
        }
    }

    #[cfg(test)]
    fn from_stamped_receiver(receiver: tokio::sync::mpsc::UnboundedReceiver<StampedKey>) -> Self {
        Self {
            receiver: Some(receiver),
            control: None,
            epoch: 0,
        }
    }

    fn ensure_reader(&mut self) {
        if self.receiver.is_none() {
            let (key_tx, key_rx) = tokio::sync::mpsc::unbounded_channel();
            let (control_tx, control_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut epoch = 0;
                loop {
                    while let Ok(control) = control_rx.try_recv() {
                        match control {
                            ReaderControl::Handoff {
                                epoch: next_epoch,
                                acknowledged,
                            } => {
                                while ratatui::crossterm::event::poll(std::time::Duration::ZERO)
                                    .unwrap_or(false)
                                {
                                    if ratatui::crossterm::event::read().is_err() {
                                        break;
                                    }
                                }
                                epoch = next_epoch;
                                let _ = acknowledged.send(());
                            }
                        }
                    }
                    match ratatui::crossterm::event::poll(std::time::Duration::from_millis(20)) {
                        Ok(true) => match ratatui::crossterm::event::read() {
                            Ok(ratatui::crossterm::event::Event::Key(key))
                                if key.kind != ratatui::crossterm::event::KeyEventKind::Release =>
                            {
                                if key_tx.send(StampedKey { epoch, key }).is_err() {
                                    return;
                                }
                            }
                            Ok(_) => {}
                            Err(_) => return,
                        },
                        Ok(false) => {}
                        Err(_) => return,
                    }
                }
            });
            self.receiver = Some(key_rx);
            self.control = Some(control_tx);
        }
    }

    fn receiver(&mut self) -> &mut tokio::sync::mpsc::UnboundedReceiver<StampedKey> {
        self.ensure_reader();
        match self.receiver.as_mut() {
            Some(receiver) => receiver,
            None => unreachable!("workspace reader initialization is synchronous"),
        }
    }

    pub async fn recv(&mut self) -> Option<KeyEvent> {
        let epoch = self.epoch;
        loop {
            let stamped = self.receiver().recv().await?;
            if stamped.epoch == epoch {
                return Some(stamped.key);
            }
        }
    }

    pub fn try_recv(
        &mut self,
    ) -> std::result::Result<KeyEvent, tokio::sync::mpsc::error::TryRecvError> {
        let epoch = self.epoch;
        loop {
            match self.receiver().try_recv() {
                Ok(stamped) if stamped.epoch == epoch => return Ok(stamped.key),
                Ok(_) => continue,
                Err(error) => return Err(error),
            }
        }
    }

    /// Establish an input boundary between nested editors/routes. Keys that
    /// were already queued for the route being closed must never become an
    /// action in the route that regains focus.
    pub async fn handoff(&mut self) {
        self.ensure_reader();
        let next_epoch = self.epoch.saturating_add(1);
        if let Some(control) = &self.control {
            let (acknowledged, ack) = tokio::sync::oneshot::channel();
            if control
                .send(ReaderControl::Handoff {
                    epoch: next_epoch,
                    acknowledged,
                })
                .is_ok()
            {
                let _ = ack.await;
            }
        }
        self.epoch = next_epoch;
    }

    /// Wait for the plain-terminal acknowledgement used around browser OAuth
    /// without creating a second stdin reader beside the workspace reader.
    pub async fn wait_for_enter(&mut self) -> bool {
        while let Some(key) = self.recv().await {
            if matches!(key.code, KeyCode::Enter) {
                return true;
            }
            if matches!(key.code, KeyCode::Esc) {
                return false;
            }
        }
        false
    }
}

/// State that survives every route handoff in one terminal workspace. Chat is
/// deliberately outside the route-local event loop so opening Settings or
/// Conversations cannot recreate its draft, cursor, attachments, transcript,
/// or resume sequence.
pub struct WorkspaceSession {
    pub workspace: WorkspaceState,
    pub chat: crate::tui::App,
    pub chat_transport: Option<ChatTransportContext>,
    pub input: WorkspaceInput,
    /// Exact Daemon owner selected for Settings. This may name a remote device
    /// and therefore must not be reconstructed from the local CLI config.
    pub daemon_device_id: String,
}

impl WorkspaceSession {
    pub fn new(route: Route) -> Self {
        Self {
            workspace: WorkspaceState::new(route),
            chat: crate::tui::App::new("connecting…"),
            chat_transport: None,
            input: WorkspaceInput::terminal(),
            daemon_device_id: crate::device_id(),
        }
    }

    pub fn with_daemon_device_id(mut self, device_id: impl Into<String>) -> Self {
        self.daemon_device_id = device_id.into();
        self
    }

    pub fn begin_chat_reconnect(&mut self) {
        self.chat_transport = None;
        self.workspace.reduce(Action::Connect);
    }

    pub fn activate_chat_transport(&mut self, context: ChatTransportContext) {
        activate_chat_transport(&mut self.workspace, &mut self.chat_transport, context);
    }

    pub fn chat_submission_enabled(&self) -> bool {
        chat_submission_enabled(&self.workspace, self.chat_transport.as_ref())
    }
}

pub fn activate_chat_transport(
    workspace: &mut WorkspaceState,
    slot: &mut Option<ChatTransportContext>,
    context: ChatTransportContext,
) {
    workspace.context.profile = Some(context.profile.clone());
    workspace.context.endpoint = Some(context.endpoint.clone());
    workspace.context.server_identity = context.server_identity.clone();
    workspace.context.server_version = context.server_version.clone();
    workspace.context.provider = context.provider.clone();
    workspace.context.model = context.model.clone();
    *slot = Some(context);
    workspace.reduce(Action::Connected);
}

pub fn chat_submission_enabled(
    workspace: &WorkspaceState,
    transport: Option<&ChatTransportContext>,
) -> bool {
    let Some(transport) = transport else {
        return false;
    };
    matches!(workspace.route, Route::Chat)
        && matches!(workspace.connection, ConnectionState::Connected)
        && workspace.context.profile.as_deref() == Some(transport.profile.as_str())
        && workspace.context.endpoint.as_deref() == Some(transport.endpoint.as_str())
        && workspace.context.server_identity == transport.server_identity
        && workspace.context.server_version == transport.server_version
        && workspace.context.provider == transport.provider
        && workspace.context.model == transport.model
}

impl WorkspaceState {
    pub fn new(route: Route) -> Self {
        let owners = [
            Owner::Cli,
            Owner::Daemon,
            Owner::Server,
            Owner::ProvidersAndModels,
        ]
        .into_iter()
        .map(|owner| (owner, OwnerState::Loading))
        .collect();
        Self {
            route,
            history: Vec::new(),
            connection: ConnectionState::Connecting,
            context: WorkspaceContext::default(),
            owners,
            notices: Vec::new(),
            palette_query: String::new(),
            palette_selected: 0,
            next_notice_id: 1,
        }
    }

    pub fn reduce(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Navigate(route) => {
                if route != self.route {
                    self.history.push(self.route.clone());
                    self.route = route;
                }
            }
            Action::Back => {
                if let Some(route) = self.history.pop() {
                    self.route = route;
                }
            }
            Action::OpenHelp => {
                let current = self.route.clone();
                self.history.push(current.clone());
                self.route = Route::Modal(Modal::Help {
                    for_route: Box::new(current),
                });
            }
            Action::OpenCommandPalette => {
                self.history.push(self.route.clone());
                self.route = Route::CommandPalette;
                self.palette_query.clear();
                self.palette_selected = 0;
            }
            Action::Connect => {
                self.connection = ConnectionState::Connecting;
                return vec![Effect::ConnectCurrentProfile];
            }
            Action::Connected => self.connection = ConnectionState::Connected,
            Action::ConnectionLost {
                attempt,
                backoff_ms,
            } => {
                self.connection = ConnectionState::Reconnecting {
                    attempt,
                    backoff_ms,
                }
            }
            Action::AuthenticationRequired => {
                self.connection = ConnectionState::AuthenticationRequired
            }
            Action::Offline(reason) => self.connection = ConnectionState::Offline { reason },
            Action::OwnerLoading(owner) => {
                self.owners.insert(owner, OwnerState::Loading);
            }
            Action::OwnerLoaded { owner, snapshot } => {
                self.owners.insert(owner, OwnerState::Available(snapshot));
            }
            Action::StageOwner { owner, snapshot } => {
                self.owners.insert(owner, OwnerState::Dirty(snapshot));
            }
            Action::ApplyOwner(owner) => {
                if let Some(OwnerState::Dirty(snapshot)) = self.owners.get(&owner).cloned() {
                    self.owners.insert(owner, OwnerState::Applying(snapshot));
                    return vec![Effect::ApplyOwner(owner)];
                }
            }
            Action::OwnerApplied { owner, snapshot } => {
                self.owners.insert(owner, OwnerState::Available(snapshot));
            }
            Action::OwnerConflict { owner, error } => {
                if let Some(snapshot) = editable_snapshot(self.owners.get(&owner)) {
                    self.owners
                        .insert(owner, OwnerState::Conflict(snapshot, error));
                }
            }
            Action::OwnerFailed { owner, error } => {
                if let Some(snapshot) = editable_snapshot(self.owners.get(&owner)) {
                    self.owners
                        .insert(owner, OwnerState::Failed(snapshot, error));
                }
            }
            Action::OwnerUnavailable { owner, error } => {
                self.owners.insert(owner, OwnerState::Unavailable(error));
            }
            Action::PushNotice(mut notice) => {
                self.assign_notice_id(&mut notice);
                self.notices.push(notice);
            }
            Action::TransientStatus(summary) => {
                self.notices
                    .retain(|notice| notice.persistence != NoticePersistence::Transient);
                let mut notice = Notice::transient(summary);
                self.assign_notice_id(&mut notice);
                self.notices.push(notice);
            }
            Action::DismissNotice(id) => self.notices.retain(|notice| notice.id != id),
            Action::ResolveNotices(summary) => {
                self.notices.retain(|notice| notice.summary != summary)
            }
        }
        Vec::new()
    }

    fn assign_notice_id(&mut self, notice: &mut Notice) {
        if notice.id == 0 {
            notice.id = self.next_notice_id;
            self.next_notice_id = self.next_notice_id.saturating_add(1);
        }
    }
}

fn editable_snapshot(state: Option<&OwnerState<OwnerSnapshot>>) -> Option<OwnerSnapshot> {
    match state {
        Some(OwnerState::Available(snapshot))
        | Some(OwnerState::Dirty(snapshot))
        | Some(OwnerState::Applying(snapshot))
        | Some(OwnerState::Conflict(snapshot, _))
        | Some(OwnerState::Failed(snapshot, _)) => Some(snapshot.clone()),
        Some(OwnerState::Loading | OwnerState::Unavailable(_)) | None => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyContext {
    pub turn_in_flight: bool,
    pub has_unsent_input: bool,
    pub has_dirty_owner: bool,
    pub text_input_focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    Forward,
    Consumed(Vec<Effect>),
    ExitRequested,
}

pub fn on_key(state: &mut WorkspaceState, key: KeyEvent, context: KeyContext) -> KeyOutcome {
    if matches!(state.route, Route::CommandPalette) {
        return palette_key(state, key);
    }
    if matches!(state.route, Route::Modal(Modal::ConfirmExit))
        && matches!(key.code, KeyCode::Enter | KeyCode::Char('y' | 'Y'))
    {
        return KeyOutcome::ExitRequested;
    }
    if matches!(state.route, Route::Modal(Modal::ConfirmExit))
        && matches!(key.code, KeyCode::Char('n' | 'N'))
    {
        return KeyOutcome::Consumed(state.reduce(Action::Back));
    }
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(character) if character.eq_ignore_ascii_case(&'k'))
    {
        return KeyOutcome::Consumed(state.reduce(Action::OpenCommandPalette));
    }
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(character) if character.eq_ignore_ascii_case(&'c'))
    {
        if context.turn_in_flight {
            return KeyOutcome::Consumed(vec![Effect::CancelTurn]);
        }
        if context.has_unsent_input || context.has_dirty_owner {
            return KeyOutcome::Consumed(
                state.reduce(Action::Navigate(Route::Modal(Modal::ConfirmExit))),
            );
        }
        return KeyOutcome::ExitRequested;
    }
    if key.code == KeyCode::Char('?') && !context.text_input_focused {
        return KeyOutcome::Consumed(state.reduce(Action::OpenHelp));
    }
    if key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Char(character) if character.eq_ignore_ascii_case(&'r'))
    {
        if let Some(notice) = visible_notice(state) {
            return KeyOutcome::Consumed(vec![Effect::RetryNotice(notice.id)]);
        }
        return KeyOutcome::Consumed(Vec::new());
    }
    if key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Char(character) if character.eq_ignore_ascii_case(&'d'))
    {
        if let Some(id) = visible_notice(state).map(|notice| notice.id) {
            state.reduce(Action::DismissNotice(id));
        }
        return KeyOutcome::Consumed(Vec::new());
    }
    if key.code == KeyCode::Esc
        && (matches!(
            state.route,
            Route::Modal(_) | Route::CommandPalette | Route::ConnectionPicker
        ) || !state.history.is_empty())
    {
        return KeyOutcome::Consumed(state.reduce(Action::Back));
    }
    if key.code == KeyCode::Esc && (context.has_unsent_input || context.has_dirty_owner) {
        return KeyOutcome::Consumed(
            state.reduce(Action::Navigate(Route::Modal(Modal::ConfirmExit))),
        );
    }
    KeyOutcome::Forward
}

fn palette_key(state: &mut WorkspaceState, key: KeyEvent) -> KeyOutcome {
    match key.code {
        KeyCode::Esc => KeyOutcome::Consumed(state.reduce(Action::Back)),
        KeyCode::Backspace => {
            state.palette_query.pop();
            state.palette_selected = 0;
            KeyOutcome::Consumed(Vec::new())
        }
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            state.palette_query.push(character);
            state.palette_selected = 0;
            KeyOutcome::Consumed(Vec::new())
        }
        KeyCode::Up => {
            state.palette_selected = state.palette_selected.saturating_sub(1);
            KeyOutcome::Consumed(Vec::new())
        }
        KeyCode::Down => {
            let count = filtered_palette_commands(state).len();
            state.palette_selected = (state.palette_selected + 1).min(count.saturating_sub(1));
            KeyOutcome::Consumed(Vec::new())
        }
        KeyCode::Enter => run_palette_selection(state),
        _ => KeyOutcome::Consumed(Vec::new()),
    }
}

fn filtered_palette_commands(state: &WorkspaceState) -> Vec<PaletteCommand> {
    let query = state.palette_query.to_ascii_lowercase();
    PALETTE_COMMANDS
        .into_iter()
        .filter(|command| command.label().to_ascii_lowercase().contains(&query))
        .collect()
}

fn run_palette_selection(state: &mut WorkspaceState) -> KeyOutcome {
    let Some(command) = filtered_palette_commands(state)
        .get(state.palette_selected)
        .copied()
    else {
        return KeyOutcome::Consumed(Vec::new());
    };
    let previous = state.history.pop().unwrap_or(Route::Chat);
    let (route, effects) = match command {
        PaletteCommand::Chat => (Route::Chat, Vec::new()),
        PaletteCommand::Conversations => (Route::Conversations, Vec::new()),
        PaletteCommand::Settings => (Route::Settings(SettingsPage::Connection), Vec::new()),
        PaletteCommand::SwitchProfile => (Route::ConnectionPicker, Vec::new()),
        PaletteCommand::Reconnect => (previous.clone(), vec![Effect::ConnectCurrentProfile]),
        PaletteCommand::Doctor => (previous.clone(), vec![Effect::RunDoctor]),
        PaletteCommand::DismissNotice => {
            if let Some(id) = visible_notice(state).map(|notice| notice.id) {
                state.reduce(Action::DismissNotice(id));
            }
            (previous.clone(), Vec::new())
        }
    };
    if route != previous {
        state.history.push(previous);
    }
    state.route = route;
    KeyOutcome::Consumed(effects)
}

pub fn render(
    frame: &mut Frame,
    state: &WorkspaceState,
    render_content: impl FnOnce(&mut Frame, Rect),
) {
    const MIN_WIDTH: u16 = 50;
    const MIN_HEIGHT: u16 = 16;
    if frame.area().width < MIN_WIDTH || frame.area().height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(format!(
                "Terminal too small\n{}x{}; need {MIN_WIDTH}x{MIN_HEIGHT}\n? help · Esc/Ctrl+C exit",
                frame.area().width,
                frame.area().height
            )),
            frame.area(),
        );
        return;
    }
    let notice_height = if state.notices.is_empty() { 0 } else { 4 };
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(notice_height),
            Constraint::Min(1),
            Constraint::Length(4),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new(header_line(state, regions[0].width.saturating_sub(2)))
            .block(Block::bordered().title("Fleety")),
        regions[0],
    );
    if notice_height > 0 {
        render_notice(frame, state, regions[1]);
    }
    match &state.route {
        Route::Modal(Modal::Help { for_route }) => render_help(frame, for_route, regions[2]),
        Route::Modal(Modal::ConfirmExit) => frame.render_widget(
            Paragraph::new("Unsaved or unsent work remains. Confirm before exiting.")
                .block(Block::bordered().title("Confirm exit"))
                .wrap(Wrap { trim: false }),
            regions[2],
        ),
        Route::Modal(Modal::ResolveDirtyProfileSwitch {
            old_profile,
            new_profile,
        }) => frame.render_widget(
            Paragraph::new(format!(
                "Profile '{}' has staged remote changes.\n\nApply, Discard, or Cancel before switching to '{}'.\n\nA: Apply · D: Discard · C/Esc: Cancel",
                crate::terminal_safe_text(old_profile),
                crate::terminal_safe_text(new_profile)
            ))
            .block(Block::bordered().title("Switch profile")),
            regions[2],
        ),
        Route::CommandPalette => render_palette(frame, state, regions[2]),
        _ => render_content(frame, regions[2]),
    }
    frame.render_widget(
        Paragraph::new(footer_line(state))
            .block(Block::bordered().title("Keys"))
            .wrap(Wrap { trim: false }),
        regions[3],
    );
}

fn header_line(state: &WorkspaceState, width: u16) -> Line<'static> {
    let profile = crate::terminal_safe_text(state.context.profile.as_deref().unwrap_or("default"));
    let connection = match &state.connection {
        ConnectionState::Connecting => "Connecting".to_string(),
        ConnectionState::Connected => "Connected".to_string(),
        ConnectionState::Reconnecting {
            attempt,
            backoff_ms,
        } => format!("Reconnecting {attempt} ({backoff_ms} ms)"),
        ConnectionState::AuthenticationRequired => "Authentication required".to_string(),
        ConnectionState::Offline { .. } => "Offline".to_string(),
    };
    let model = match (&state.context.provider, &state.context.model) {
        (Some(provider), Some(model)) => format!(
            "{}/{}",
            crate::terminal_safe_text(provider),
            crate::terminal_safe_text(model)
        ),
        (Some(provider), None) => crate::terminal_safe_text(provider),
        (None, Some(model)) => crate::terminal_safe_text(model),
        _ => "model unset".to_string(),
    };
    let route = route_label(&state.route);
    let full = format!("profile {profile}  ·  {connection}  ·  {model}  ·  {route}");
    if display_width(&full) <= usize::from(width) {
        return Line::from(full);
    }
    let separators = 9usize;
    let width = usize::from(width);
    let fixed = display_width(&connection) + display_width(route) + separators;
    let flexible = width.saturating_sub(fixed);
    let profile_width = flexible.div_ceil(2);
    let model_width = flexible / 2;
    Line::from(format!(
        "{} · {connection} · {} · {route}",
        truncate_columns(&profile, profile_width),
        truncate_columns(&model, model_width)
    ))
}

fn display_width(value: &str) -> usize {
    Line::from(value).width()
}

fn truncate_columns(value: &str, max_width: usize) -> String {
    if display_width(value) <= max_width {
        return value.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".into();
    }
    let target = max_width - 1;
    let mut result = String::new();
    for grapheme in value.graphemes(true) {
        let candidate_width = display_width(&result) + display_width(grapheme);
        if candidate_width > target {
            break;
        }
        result.push_str(grapheme);
    }
    result.push('…');
    result
}

fn route_label(route: &Route) -> &'static str {
    match route {
        Route::Chat => "Chat",
        Route::Conversations => "Conversations",
        Route::Settings(_) => "Settings",
        Route::ConnectionPicker => "Profiles",
        Route::CommandPalette => "Command palette",
        Route::Modal(Modal::Help { .. }) => "Help",
        Route::Modal(Modal::ConfirmExit) => "Confirm exit",
        Route::Modal(Modal::ResolveDirtyProfileSwitch { .. }) => "Switch profile",
    }
}

fn render_notice(frame: &mut Frame, state: &WorkspaceState, area: Rect) {
    let Some(notice) = visible_notice(state) else {
        return;
    };
    let mut text = crate::terminal_safe_text(&notice.summary);
    if let Some(details) = &notice.details {
        text.push_str(" · ");
        text.push_str(&crate::terminal_safe_text(details));
    }
    if let Some(remediation) = &notice.remediation {
        text.push_str(" · ");
        text.push_str(&crate::terminal_safe_text(remediation));
    }
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::bordered().title(match notice.severity {
                NoticeSeverity::Info => "Status",
                NoticeSeverity::Warning => "Warning",
                NoticeSeverity::Error => "Error",
            }))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn visible_notice(state: &WorkspaceState) -> Option<&Notice> {
    state
        .notices
        .iter()
        .rev()
        .find(|notice| notice.persistence == NoticePersistence::UntilResolved)
        .or_else(|| {
            state
                .notices
                .iter()
                .rev()
                .find(|notice| notice.persistence == NoticePersistence::UntilDismissed)
        })
        .or_else(|| state.notices.last())
}

fn render_help(frame: &mut Frame, for_route: &Route, area: Rect) {
    frame.render_widget(
        Paragraph::new(format!(
            "{} help\n\nEsc returns · Ctrl+K opens commands · Ctrl+C cancels an active turn before exit",
            route_label(for_route)
        ))
        .block(Block::bordered().title("Contextual help"))
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_palette(frame: &mut Frame, state: &WorkspaceState, area: Rect) {
    let commands = filtered_palette_commands(state);
    let mut lines = vec![Line::from(format!("> {}", state.palette_query))];
    lines.extend(commands.iter().enumerate().map(|(index, command)| {
        let marker = if index == state.palette_selected {
            "▶"
        } else {
            " "
        };
        Line::from(format!("{marker} {}", command.label()))
    }));
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("Command palette")),
        area,
    );
}

fn footer_line(state: &WorkspaceState) -> Line<'static> {
    let route_keys = match &state.route {
        Route::Chat => "Enter send · Esc back/cancel",
        Route::Conversations => "Enter open · Esc back",
        Route::Settings(_) => "Enter edit · a apply · Esc back",
        Route::ConnectionPicker => "Enter switch · Esc back",
        Route::CommandPalette => "type filter · Enter run · Esc back",
        Route::Modal(Modal::ConfirmExit) => "Enter/y exit · Esc/n back",
        Route::Modal(_) => "Esc back",
    };
    let notice_keys = if state.notices.is_empty() {
        ""
    } else {
        " · Alt+R retry · Alt+D dismiss"
    };
    Line::from(format!(
        "{route_keys} · ?: help · Ctrl+K: commands{notice_keys}"
    ))
}

pub enum SessionResult {
    Exit,
    Continue(Box<WorkspaceSession>),
}

pub async fn run(initial_route: Route) -> agent_core::Result<()> {
    run_session(WorkspaceSession::new(initial_route)).await
}

pub async fn run_session(mut session: WorkspaceSession) -> agent_core::Result<()> {
    loop {
        let result = if matches!(&session.workspace.route, Route::Settings(_)) {
            crate::config_panel::run(session).await?
        } else {
            crate::run_tui(session).await?
        };
        match result {
            SessionResult::Exit => return Ok(()),
            SessionResult::Continue(next) => session = *next,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;

    #[test]
    fn terminal_entry_matrix_is_explicit_and_safe() {
        assert_eq!(
            choose_entry(InteractiveEntry::Bare, true, true),
            EntryDecision::Workspace(Route::Chat)
        );
        assert_eq!(
            choose_entry(InteractiveEntry::Bare, false, true),
            EntryDecision::Help
        );
        assert_eq!(
            choose_entry(InteractiveEntry::Bare, true, false),
            EntryDecision::Help
        );
        assert_eq!(
            choose_entry(InteractiveEntry::Chat, false, false),
            EntryDecision::Workspace(Route::Chat)
        );
        assert_eq!(
            choose_entry(InteractiveEntry::Settings, true, true),
            EntryDecision::Workspace(Route::Settings(SettingsPage::Connection))
        );
    }

    #[test]
    fn workspace_session_preserves_multiline_draft_cursor_and_attachment_across_routes() {
        let mut session = WorkspaceSession::new(Route::Chat);
        session.chat.input.set_text("first line\n第二行".into());
        session.chat.input.left();
        session.chat.input.left();
        session.chat.attach(fleety_protocol::WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("cG5n".into()),
            url: None,
            name: Some("draft.png".into()),
        });
        let cursor = session.chat.input.cursor_row_col();
        let attachment = session.chat.pending_attachments[0].clone();

        session
            .workspace
            .reduce(Action::Navigate(Route::Conversations));
        session.workspace.reduce(Action::Back);
        session
            .workspace
            .reduce(Action::Navigate(Route::Settings(SettingsPage::Connection)));
        session.workspace.reduce(Action::Back);

        assert_eq!(session.workspace.route, Route::Chat);
        assert_eq!(session.chat.input.text(), "first line\n第二行");
        assert_eq!(session.chat.input.cursor_row_col(), cursor);
        assert_eq!(session.chat.pending_attachments, vec![attachment]);
    }

    #[test]
    fn workspace_session_preserves_explicit_remote_daemon_owner_across_routes() {
        let mut session = WorkspaceSession::new(Route::Settings(SettingsPage::Daemon))
            .with_daemon_device_id("remote-B");
        session.workspace.reduce(Action::Navigate(Route::Chat));
        session
            .workspace
            .reduce(Action::Navigate(Route::Settings(SettingsPage::Daemon)));
        assert_eq!(session.daemon_device_id, "remote-B");
    }

    #[tokio::test]
    async fn workspace_input_is_one_ordered_stream_across_route_handoffs() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut input = WorkspaceInput::from_receiver(rx);
        for code in [KeyCode::Char('p'), KeyCode::Char('a'), KeyCode::Esc] {
            tx.send(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
        }

        assert_eq!(
            input.recv().await.map(|key| key.code),
            Some(KeyCode::Char('p'))
        );
        // Moving the owner models Chat -> Settings -> Provider handoffs. No
        // route gets a second reader. A handoff boundary prevents stale keys
        // from becoming actions in the route that regains focus.
        let mut input_after_handoff = input;
        input_after_handoff.handoff().await;
        assert!(matches!(
            input_after_handoff.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn oauth_acknowledgement_uses_workspace_input_and_consumes_enter() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut input = WorkspaceInput::from_receiver(rx);
        tx.send(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();
        tx.send(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(input.wait_for_enter().await);
        assert!(matches!(
            input.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn handoff_rejects_an_old_epoch_key_that_arrives_after_the_boundary() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut input = WorkspaceInput::from_stamped_receiver(rx);

        input.handoff().await;
        tx.send(StampedKey {
            epoch: 0,
            key: KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        })
        .unwrap();
        tx.send(StampedKey {
            epoch: 1,
            key: KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        })
        .unwrap();

        assert_eq!(input.recv().await.map(|key| key.code), Some(KeyCode::Esc));
    }

    #[test]
    fn chat_submission_requires_header_and_transport_identity_to_match_atomically() {
        let mut session = WorkspaceSession::new(Route::Chat);
        session.activate_chat_transport(ChatTransportContext {
            profile: "A".into(),
            endpoint: "ws://a.test:8787".into(),
            server_identity: Some("server-a".into()),
            server_version: Some("1.0.0".into()),
            provider: Some("codex".into()),
            model: Some("gpt-a".into()),
        });
        assert!(session.chat_submission_enabled());

        session.workspace.context.profile = Some("B".into());
        assert!(
            !session.chat_submission_enabled(),
            "changing the visible profile must disable a stale A transport"
        );

        session.begin_chat_reconnect();
        assert!(matches!(
            session.workspace.connection,
            ConnectionState::Connecting
        ));
        assert!(!session.chat_submission_enabled());
        session.activate_chat_transport(ChatTransportContext {
            profile: "B".into(),
            endpoint: "ws://b.test:8787".into(),
            server_identity: Some("server-b".into()),
            server_version: Some("2.0.0".into()),
            provider: Some("openai".into()),
            model: Some("gpt-b".into()),
        });

        assert!(session.chat_submission_enabled());
        assert_eq!(session.workspace.context.profile.as_deref(), Some("B"));
        assert_eq!(
            session.workspace.context.server_identity.as_deref(),
            Some("server-b")
        );
        assert_eq!(
            session.workspace.context.provider.as_deref(),
            Some("openai")
        );
        assert_eq!(session.workspace.context.model.as_deref(), Some("gpt-b"));
    }

    #[test]
    fn route_history_modal_and_back_are_one_consistent_transition() {
        let mut state = WorkspaceState::new(Route::Chat);
        assert_eq!(state.reduce(Action::Navigate(Route::Conversations)), vec![]);
        assert_eq!(state.route, Route::Conversations);
        state.reduce(Action::OpenHelp);
        assert!(matches!(state.route, Route::Modal(Modal::Help { .. })));
        state.reduce(Action::Back);
        assert_eq!(state.route, Route::Conversations);
        state.reduce(Action::Back);
        assert_eq!(state.route, Route::Chat);

        let mut dirty = WorkspaceState::new(Route::Settings(SettingsPage::Server));
        assert_eq!(
            on_key(
                &mut dirty,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyContext {
                    has_dirty_owner: true,
                    ..KeyContext::default()
                },
            ),
            KeyOutcome::Consumed(Vec::new())
        );
        assert_eq!(dirty.route, Route::Modal(Modal::ConfirmExit));
        on_key(
            &mut dirty,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            KeyContext::default(),
        );
        assert_eq!(dirty.route, Route::Settings(SettingsPage::Server));
    }

    #[test]
    fn connection_and_owner_transitions_emit_effects_without_io() {
        let mut state = WorkspaceState::new(Route::Settings(SettingsPage::Server));
        let effects = state.reduce(Action::Connect);
        assert_eq!(effects, vec![Effect::ConnectCurrentProfile]);
        assert_eq!(state.connection, ConnectionState::Connecting);

        state.reduce(Action::ConnectionLost {
            attempt: 2,
            backoff_ms: 500,
        });
        assert_eq!(
            state.connection,
            ConnectionState::Reconnecting {
                attempt: 2,
                backoff_ms: 500
            }
        );

        state.reduce(Action::OwnerLoaded {
            owner: Owner::Server,
            snapshot: OwnerSnapshot::new("r1"),
        });
        state.reduce(Action::StageOwner {
            owner: Owner::Server,
            snapshot: OwnerSnapshot::new("r1-edited"),
        });
        assert!(matches!(
            state.owners.get(&Owner::Server),
            Some(OwnerState::Dirty(_))
        ));
        assert_eq!(
            state.reduce(Action::ApplyOwner(Owner::Server)),
            vec![Effect::ApplyOwner(Owner::Server)]
        );
    }

    #[test]
    fn unresolved_errors_survive_transient_status_and_navigation() {
        let mut state = WorkspaceState::new(Route::Settings(SettingsPage::ProvidersAndModels));
        let error = Notice::error("Catalog failed")
            .details("backend unavailable")
            .remediation("Retry or enter a model ID");
        state.reduce(Action::PushNotice(error));
        let id = state.notices[0].id;
        state.reduce(Action::TransientStatus("Connected".into()));
        state.reduce(Action::OpenHelp);
        state.reduce(Action::Back);

        assert!(state.notices.iter().any(|notice| notice.id == id));
        assert!(state
            .notices
            .iter()
            .any(|notice| notice.summary == "Connected"));
        state.reduce(Action::DismissNotice(id));
        assert!(!state.notices.iter().any(|notice| notice.id == id));
    }

    #[test]
    fn failed_or_conflicted_apply_retains_the_staged_owner_snapshot() {
        let mut state = WorkspaceState::new(Route::Settings(SettingsPage::Server));
        let staged = OwnerSnapshot::new("r1-edited");
        state.reduce(Action::StageOwner {
            owner: Owner::Server,
            snapshot: staged.clone(),
        });
        state.reduce(Action::ApplyOwner(Owner::Server));
        state.reduce(Action::OwnerConflict {
            owner: Owner::Server,
            error: WorkspaceError {
                kind: "conflict".into(),
                message: "revision changed".into(),
                remediation: Some("Reload or retry".into()),
            },
        });
        assert!(matches!(
            state.owners.get(&Owner::Server),
            Some(OwnerState::Conflict(snapshot, _)) if snapshot == &staged
        ));

        state.reduce(Action::OwnerFailed {
            owner: Owner::Server,
            error: WorkspaceError {
                kind: "transport".into(),
                message: "offline".into(),
                remediation: Some("Reconnect".into()),
            },
        });
        assert!(matches!(
            state.owners.get(&Owner::Server),
            Some(OwnerState::Failed(snapshot, _)) if snapshot == &staged
        ));
        assert!(state.reduce(Action::ApplyOwner(Owner::Cli)).is_empty());
    }

    #[test]
    fn reducer_is_deterministic_without_global_notice_state() {
        let mut left = WorkspaceState::new(Route::Chat);
        let mut right = WorkspaceState::new(Route::Chat);
        for state in [&mut left, &mut right] {
            state.reduce(Action::PushNotice(Notice::error("offline")));
            state.reduce(Action::TransientStatus("retrying".into()));
            state.reduce(Action::ConnectionLost {
                attempt: 1,
                backoff_ms: 250,
            });
        }
        assert_eq!(left, right);
    }

    #[test]
    fn shell_keeps_context_notice_route_and_keys_visible() {
        let mut state = WorkspaceState::new(Route::Chat);
        state.context.profile = Some("office".into());
        state.context.endpoint = Some("ws://office.test:8787".into());
        state.context.provider = Some("codex".into());
        state.context.model = Some("gpt-5".into());
        state.connection = ConnectionState::Reconnecting {
            attempt: 2,
            backoff_ms: 500,
        };
        state.reduce(Action::PushNotice(
            Notice::error("Catalog failed")
                .details("backend unavailable")
                .remediation("Retry or enter a model ID"),
        ));
        state.reduce(Action::TransientStatus("Connected".into()));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("terminal");
        terminal
            .draw(|frame| {
                render(frame, &state, |frame, area| {
                    frame.render_widget(ratatui::widgets::Paragraph::new("chat content"), area);
                })
            })
            .expect("draw");
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for expected in [
            "office",
            "Reconnecting 2",
            "codex/gpt-5",
            "Chat",
            "Catalog failed",
            "Retry or enter a model ID",
            "chat content",
            "Ctrl+K",
        ] {
            assert!(
                content.contains(expected),
                "missing {expected:?}: {content}"
            );
        }
    }

    #[test]
    fn shell_sanitizes_remote_context_and_notice_at_the_render_boundary() {
        let mut state = WorkspaceState::new(Route::Chat);
        state.context.profile = Some("bad\u{1b}]52;c;STEAL\u{7}\nprofile".into());
        state.context.provider = Some("codex\rforged".into());
        state.context.model = Some("gpt\u{1b}[31m".into());
        state.reduce(Action::PushNotice(
            Notice::error("failed\nforged")
                .details("wss://u:p@host/x?token=NOTICE#tail")
                .remediation("retry\u{7} now"),
        ));
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("terminal");
        terminal
            .draw(|frame| render(frame, &state, |_frame, _area| {}))
            .expect("draw");
        let content = buffer_text(&terminal);

        for secret in ["NOTICE", "u:p", "#tail"] {
            assert!(!content.contains(secret), "leaked {secret}: {content}");
        }
        assert!(!content.contains('\u{1b}'), "{content}");
        assert!(!content.contains('\u{7}'), "{content}");
        assert!(
            content.contains("\\u{1b}]52;c;STEAL\\u{7}\\nprofile"),
            "{content}"
        );
        assert!(content.contains("failed\\nforged"), "{content}");
        assert!(content.contains("token=<redacted>"), "{content}");
    }

    fn buffer_text(terminal: &ratatui::Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn minimum_viewport_keeps_every_contextual_key_visible() {
        let mut state = WorkspaceState::new(Route::Chat);
        state.context.profile = Some("very-long-profile-辦公室👨‍👩‍👧‍👦".into());
        state.context.provider = Some("provider-with-a-very-long-name".into());
        state.context.model = Some("model-with-a-very-long-name".into());
        state.reduce(Action::Connected);
        state.reduce(Action::PushNotice(
            Notice::error("offline").remediation("retry the connection"),
        ));
        let mut terminal = Terminal::new(TestBackend::new(50, 16)).expect("terminal");
        terminal
            .draw(|frame| {
                render(frame, &state, |frame, area| {
                    frame.render_widget(Paragraph::new("chat"), area);
                });
            })
            .expect("draw");
        let content = buffer_text(&terminal);
        for expected in [
            "Connected",
            "Chat",
            "Enter",
            "Esc",
            "?",
            "Ctrl+K",
            "Alt+R",
            "Alt+D",
        ] {
            assert!(
                content.contains(expected),
                "missing {expected:?}: {content}"
            );
        }
    }

    #[test]
    fn truncation_preserves_grapheme_clusters_and_route_context() {
        assert_eq!(truncate_columns("A👨‍👩‍👧‍👦B", 3), "A…");
        assert_eq!(truncate_columns("e\u{301}xyz", 2), "e\u{301}…");

        let mut state = WorkspaceState::new(Route::Chat);
        state.context.profile = Some("profile-👨‍👩‍👧‍👦-with-a-very-long-name".into());
        state.context.provider = Some("provider-旗幟🇹🇼-very-long".into());
        state.context.model = Some("model-with-a-very-long-name".into());
        state.reduce(Action::Connected);
        let rendered = header_line(&state, 78).to_string();
        assert!(rendered.contains("Connected"), "{rendered}");
        assert!(rendered.contains("Chat"), "{rendered}");
        assert!(!rendered.ends_with('\u{200d}'), "{rendered}");
    }

    #[test]
    fn responsive_unicode_workspace_render_matrix_matches_semantic_golden() {
        let sizes = [(120, 30), (80, 24), (50, 16)];
        let routes = [
            Route::Chat,
            Route::Conversations,
            Route::Settings(SettingsPage::ProvidersAndModels),
            Route::CommandPalette,
            Route::Modal(Modal::Help {
                for_route: Box::new(Route::Chat),
            }),
        ];
        let mut golden = String::new();
        for (width, height) in sizes {
            for route in &routes {
                let mut state = WorkspaceState::new(route.clone());
                state.context.profile = Some("辦公室🚀".into());
                state.context.endpoint = Some(
                    "wss://極長的伺服器端點.example.test:8787/一段很長但不能破壞版面的路徑".into(),
                );
                state.context.provider = Some("供應商".into());
                state.context.model = Some("模型-α".into());
                state.reduce(Action::Connected);
                let mut app = crate::tui::App::new("準備完成 ✅");
                app.push("you", "ASCII + 中文 + emoji 🧭");
                app.input.set_text("草稿🙂".into());
                app.conversations = vec![crate::tui::ConversationSummary {
                    conversation_id: "對話-1".into(),
                    last_ts_secs: 1,
                    events: 2,
                    preview: "測試對話 💬".into(),
                }];
                app.conversations_status = "1 conversation".into();
                let mut terminal =
                    ratatui::Terminal::new(TestBackend::new(width, height)).expect("terminal");
                terminal
                    .draw(|frame| {
                        render(frame, &state, |frame, area| match state.route {
                            Route::Chat => crate::tui::render_in_area(frame, &app, area),
                            Route::Conversations => {
                                crate::tui::render_conversations_in_area(frame, &app, area)
                            }
                            Route::Settings(_) => frame
                                .render_widget(Paragraph::new("Server settings · 中文值 ⚙"), area),
                            _ => {}
                        });
                    })
                    .expect("responsive render");
                let content = buffer_text(&terminal);
                assert!(
                    !content.contains('�'),
                    "{width}x{height} {route:?}: {content}"
                );
                assert!(content.contains("Fleety"), "{width}x{height} {route:?}");
                assert!(content.contains("Keys"), "{width}x{height} {route:?}");
                assert!(content.contains('辦'), "{width}x{height} {route:?}");
                assert!(content.contains('🚀'), "{width}x{height} {route:?}");
                golden.push_str(&format!(
                    "{width}x{height} {route:?}:shell={} route={} unicode={} replacement={}\n",
                    content.contains("Fleety") && content.contains("Keys"),
                    content.contains(route_label(route)),
                    content.contains('辦') && content.contains('🚀'),
                    content.contains('�')
                ));
            }
        }
        assert_eq!(
            golden,
            "120x30 Chat:shell=true route=true unicode=true replacement=false\n\
120x30 Conversations:shell=true route=true unicode=true replacement=false\n\
120x30 Settings(ProvidersAndModels):shell=true route=true unicode=true replacement=false\n\
120x30 CommandPalette:shell=true route=true unicode=true replacement=false\n\
120x30 Modal(Help { for_route: Chat }):shell=true route=true unicode=true replacement=false\n\
80x24 Chat:shell=true route=true unicode=true replacement=false\n\
80x24 Conversations:shell=true route=true unicode=true replacement=false\n\
80x24 Settings(ProvidersAndModels):shell=true route=true unicode=true replacement=false\n\
80x24 CommandPalette:shell=true route=true unicode=true replacement=false\n\
80x24 Modal(Help { for_route: Chat }):shell=true route=true unicode=true replacement=false\n\
50x16 Chat:shell=true route=true unicode=true replacement=false\n\
50x16 Conversations:shell=true route=true unicode=true replacement=false\n\
50x16 Settings(ProvidersAndModels):shell=true route=true unicode=true replacement=false\n\
50x16 CommandPalette:shell=true route=true unicode=true replacement=false\n\
50x16 Modal(Help { for_route: Chat }):shell=true route=true unicode=true replacement=false\n"
        );
    }

    #[test]
    fn below_minimum_terminal_renders_safe_help_and_exit_screen_without_content() {
        for (width, height) in [(49, 16), (50, 15), (20, 5), (1, 1)] {
            let state = WorkspaceState::new(Route::Chat);
            let mut terminal =
                ratatui::Terminal::new(TestBackend::new(width, height)).expect("terminal");
            let content_called = std::cell::Cell::new(false);
            terminal
                .draw(|frame| {
                    render(frame, &state, |_, _| content_called.set(true));
                })
                .expect("small terminal render");
            let content = buffer_text(&terminal);
            assert!(
                !content_called.get(),
                "normal content must not render below minimum"
            );
            if width >= 20 && height >= 5 {
                assert!(
                    content.contains("Terminal too small"),
                    "{width}x{height}: {content}"
                );
                assert!(content.contains("50x16"), "{width}x{height}: {content}");
                assert!(content.contains('?'), "{width}x{height}: {content}");
                assert!(content.contains("Esc"), "{width}x{height}: {content}");
            }
            assert!(!content.contains('�'), "{width}x{height}: {content}");
        }
    }

    #[test]
    fn workspace_key_matrix_is_modal_and_turn_consistent() {
        let mut state = WorkspaceState::new(Route::Chat);
        assert_eq!(
            on_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
                KeyContext::default(),
            ),
            KeyOutcome::Consumed(vec![])
        );
        assert!(matches!(state.route, Route::Modal(Modal::Help { .. })));
        assert_eq!(
            on_key(
                &mut state,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyContext::default(),
            ),
            KeyOutcome::Consumed(vec![])
        );
        assert_eq!(state.route, Route::Chat);

        on_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            KeyContext::default(),
        );
        assert_eq!(state.route, Route::CommandPalette);
        on_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyContext::default(),
        );
        assert_eq!(state.route, Route::Chat);

        assert_eq!(
            on_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                KeyContext {
                    turn_in_flight: true,
                    ..KeyContext::default()
                },
            ),
            KeyOutcome::Consumed(vec![Effect::CancelTurn])
        );
        assert_eq!(state.route, Route::Chat);
    }

    #[test]
    fn command_palette_filters_runs_routes_and_preserves_back_history() {
        let mut state = WorkspaceState::new(Route::Chat);
        on_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            KeyContext::default(),
        );
        for character in "switch".chars() {
            on_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                KeyContext::default(),
            );
        }
        assert_eq!(
            filtered_palette_commands(&state),
            vec![PaletteCommand::SwitchProfile]
        );
        assert_eq!(
            on_key(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                KeyContext::default(),
            ),
            KeyOutcome::Consumed(Vec::new())
        );
        assert_eq!(state.route, Route::ConnectionPicker);
        on_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyContext::default(),
        );
        assert_eq!(state.route, Route::Chat);
    }

    #[test]
    fn notice_retry_and_dismiss_use_non_text_alt_shortcuts() {
        let mut state = WorkspaceState::new(Route::Chat);
        state.reduce(Action::PushNotice(Notice::error("catalog failed")));
        let id = state.notices[0].id;
        assert_eq!(
            on_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
                KeyContext::default(),
            ),
            KeyOutcome::Consumed(vec![Effect::RetryNotice(id)])
        );
        on_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
            KeyContext::default(),
        );
        assert!(state.notices.is_empty());
    }
}
