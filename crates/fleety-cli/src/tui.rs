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

use fleety_protocol::{InterjectionDisposition, WireAttachment};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use fleety_textarea::{TextArea, TextAreaState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub last_ts_secs: u64,
    pub events: u64,
    pub preview: String,
}

pub fn parse_conversation_summaries(
    json: &str,
) -> std::result::Result<Vec<ConversationSummary>, serde_json::Error> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(ConversationSummary {
                conversation_id: row.get("conversation_id")?.as_str()?.to_string(),
                last_ts_secs: row
                    .get("last_ts_secs")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default(),
                events: row
                    .get("events")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default(),
                preview: row
                    .get("preview")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

/// Braille spinner frames, advanced on a fixed tick while a turn is in flight
/// (or during reconnection) so the wait shows motion even with no new frames.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingUserConfirmation {
    message_id: String,
    text: String,
}

/// TUI state.
pub struct App {
    pub messages: Vec<(String, String)>,
    pub input: TextArea,
    pub status: String,
    pub should_quit: bool,
    /// Attachments staged for the next `Send`. Cleared automatically when the
    /// user submits the line.
    pub pending_attachments: Vec<WireAttachment>,
    /// Whether the last message is an assistant reply still streaming in.
    streaming: bool,
    /// Index of the first message not yet handed to the terminal. Everything
    /// before it is terminal history and is never drawn again.
    emitted_upto: usize,
    /// How many bytes of the in-flight streaming reply have been handed over.
    /// Only whole markdown blocks are emitted, so the rest stays in the
    /// viewport where it can still be redrawn as it changes.
    stream_emitted_bytes: usize,
    /// Queued ahead of the conversation: the startup banner. Goes out through
    /// the same seam as everything else so a resize replays it too.
    outbox: Vec<String>,
    /// Everything handed over so far, in the form it was handed over in. Kept
    /// so a resize can re-emit it: the terminal's own reflow mangles styled
    /// output, so the whole history is reset and replayed at the new width.
    history: String,
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
    /// Raw text of locally committed user messages whose authoritative storage
    /// events have not yet been observed, oldest first. A reconnect can replay
    /// those events after the optimistic `you:` lines are already in scrollback;
    /// this lets replay confirm sends instead of printing them a second time.
    pending_user_confirmations: VecDeque<PendingUserConfirmation>,
    /// Number of leading confirmations whose turns ended with an in-band error.
    /// An error has no event sequence, so replay may still confirm them; a later
    /// successful reply retires this failed prefix before its own confirmation.
    failed_user_confirmations: usize,
    /// Whether an authoritative reply or error already ended the current turn.
    /// `Done` follows those terminal frames, but also disambiguates a `seq = 0`
    /// assistant notice that was itself the only terminal response.
    terminal_outcome_seen: bool,
    /// Whether the next `Done` terminates a reconnect/conversation Resume
    /// stream rather than a user turn. Resume and UserMessage share the same
    /// wire terminator, so this keeps fast post-reconnect sends correlated.
    resume_in_flight: bool,
    /// Whether an idle Esc is awaiting a second, confirming Esc because there is
    /// unsent input or a pending attachment. Cleared by any editing keypress.
    pub confirm_quit: bool,
    /// Server-owned conversation summaries shown by the Conversations route.
    pub conversations: Vec<ConversationSummary>,
    pub conversation_selected: usize,
    pub conversations_status: String,
}

impl App {
    pub fn new(status: impl Into<String>) -> Self {
        Self {
            messages: Vec::new(),
            input: TextArea::new(),
            status: status.into(),
            should_quit: false,
            pending_attachments: Vec::new(),
            streaming: false,
            outbox: Vec::new(),
            emitted_upto: 0,
            stream_emitted_bytes: 0,
            history: String::new(),
            pending_approvals: VecDeque::new(),
            turn_in_flight: false,
            spinner_frame: 0,
            last_conversation_id: None,
            last_seq: 0,
            pending_user_confirmations: VecDeque::new(),
            failed_user_confirmations: 0,
            terminal_outcome_seen: false,
            resume_in_flight: false,
            confirm_quit: false,
            conversations: Vec::new(),
            conversation_selected: 0,
            conversations_status: "Not loaded".into(),
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
    /// `true` when the event was newly consumed (appended or reconciled with a
    /// local optimistic send), `false` when its sequence was already observed.
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
        if role == "user"
            && self
                .pending_user_confirmations
                .front()
                .map(|confirmation| confirmation.text.as_str())
                == Some(content)
        {
            self.pending_user_confirmations.pop_front();
            self.failed_user_confirmations = self.failed_user_confirmations.saturating_sub(1);
            self.last_seq = seq;
            return true;
        }
        let display_role = match role {
            "user" => "you",
            "assistant" => "fleety",
            other => other,
        };
        self.push(display_role, content);
        if role == "assistant" {
            self.terminal_outcome_seen = true;
            self.turn_in_flight =
                self.pending_user_confirmations.len() > self.failed_user_confirmations;
        }
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
        self.terminal_outcome_seen = true;
        while self.failed_user_confirmations > 0 {
            self.pending_user_confirmations.pop_front();
            self.failed_user_confirmations -= 1;
        }
        self.pending_user_confirmations.pop_front();
        self.turn_in_flight = !self.pending_user_confirmations.is_empty();
    }

    /// Display a non-persisted assistant notice without treating it as the
    /// final reply. Mid-turn interjection acknowledgements use `seq = 0` and
    /// must not retire the user event needed for reconnect de-duplication.
    pub fn push_assistant_notice(&mut self, text: String) {
        self.push("fleety", text);
    }

    /// Apply the Server's structured response to a message sent while another
    /// turn is active. Accepted messages remain pending for replay and their
    /// eventual reply; ignored or capacity-rejected messages never become a
    /// turn, so retire only the optimistic confirmation carrying the matching
    /// client message id.
    pub fn acknowledge_interjection(
        &mut self,
        message_id: &str,
        disposition: InterjectionDisposition,
        message: String,
    ) {
        self.push("fleety", message);
        if matches!(
            disposition,
            InterjectionDisposition::Ignored | InterjectionDisposition::Rejected
        ) {
            if let Some(index) = self
                .pending_user_confirmations
                .iter()
                .position(|confirmation| confirmation.message_id == message_id)
            {
                self.pending_user_confirmations.remove(index);
                if index < self.failed_user_confirmations {
                    self.failed_user_confirmations -= 1;
                }
            }
        }
        self.turn_in_flight =
            self.pending_user_confirmations.len() > self.failed_user_confirmations;
    }

    /// Finish a user turn that produced an in-band error instead of an
    /// assistant reply. Keep its pending confirmation: the Server stored the
    /// user event before calling the provider, but the error carries no event
    /// sequence, so only a later replay can authoritatively confirm it.
    pub fn finish_error(&mut self) {
        if self.terminal_outcome_seen {
            return;
        }
        self.terminal_outcome_seen = true;
        if self.failed_user_confirmations < self.pending_user_confirmations.len() {
            self.failed_user_confirmations += 1;
        }
        self.turn_in_flight =
            self.pending_user_confirmations.len() > self.failed_user_confirmations;
    }

    /// Clear transport-scoped terminal bookkeeping before reconnecting. Failed
    /// confirmations stay queued for replay de-duplication, but their Error
    /// must not consume the first terminal frame from the replacement link.
    pub fn prepare_for_reconnect(&mut self) {
        self.turn_in_flight = false;
        self.terminal_outcome_seen = false;
        self.resume_in_flight = false;
    }

    /// Mark a successfully submitted Resume. Its Done only closes replay; it
    /// must never retire a user confirmation submitted on the new transport.
    pub fn begin_resume(&mut self) {
        self.resume_in_flight = true;
    }

    /// Consume the wire-level turn terminator. Usually an authoritative
    /// Assistant/Error already updated state; when the only reply had `seq = 0`
    /// (for example an access denial), `Done` is what makes it terminal.
    pub fn finish_done(&mut self) {
        if self.resume_in_flight {
            self.resume_in_flight = false;
            self.terminal_outcome_seen = false;
            self.turn_in_flight =
                self.pending_user_confirmations.len() > self.failed_user_confirmations;
            return;
        }
        if self.terminal_outcome_seen {
            self.terminal_outcome_seen = false;
            return;
        }
        while self.failed_user_confirmations > 0 {
            self.pending_user_confirmations.pop_front();
            self.failed_user_confirmations -= 1;
        }
        // The current seq-zero terminal response did not carry an event
        // sequence, so Done is the only authoritative retirement signal.
        self.pending_user_confirmations.pop_front();
        self.turn_in_flight =
            self.pending_user_confirmations.len() > self.failed_user_confirmations;
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
            "Message (Enter=send, Alt+Enter=newline, Ctrl+V=paste, Esc=quit)".to_string()
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

    /// The part of the conversation that is still changing, and so is still
    /// drawn by Fleety: the tail of a streaming reply past its last closed
    /// block. Empty whenever nothing is in flight.
    pub fn viewport_tail(&self) -> Vec<Line<'static>> {
        if !self.streaming {
            return Vec::new();
        }
        let Some((role, text)) = self.messages.last() else {
            return Vec::new();
        };
        let rest = &text[char_boundary(text, self.stream_emitted_bytes)..];
        if rest.trim().is_empty() {
            return Vec::new();
        }
        let mut lines = if role == "fleety" {
            crate::markdown::render(rest)
        } else {
            rest.split('\n')
                .map(|l| Line::from(l.to_string()))
                .collect()
        };
        if self.stream_emitted_bytes == 0 {
            match lines.first_mut() {
                Some(first) => first.spans.insert(0, Span::raw(format!("{role}: "))),
                None => lines.push(Line::from(format!("{role}: "))),
            }
        }
        lines
    }

    /// How tall the viewport wants to be for `width`, capped at `max`.
    ///
    /// The cap is what keeps inline mode inline: a reply that streams a huge
    /// unclosed code block would otherwise grow the viewport until it covered
    /// the screen, which is the mode we just left.
    pub fn viewport_height(&self, width: u16, max: u16) -> u16 {
        let inner_w = width.saturating_sub(2);
        let composer = self.input.desired_height(inner_w).clamp(1, MAX_INPUT_ROWS);
        let tail = u16::try_from(self.viewport_tail().len()).unwrap_or(u16::MAX);
        // composer box borders + status line
        tail.saturating_add(composer)
            .saturating_add(3)
            .clamp(4, max.max(4))
    }

    /// Queue a block to be written above the conversation, before anything else.
    pub fn announce(&mut self, block: String) {
        self.outbox.push(block);
    }

    /// Hand over everything that has settled, oldest first.
    ///
    /// A message that is complete goes over whole. A reply still streaming goes
    /// over only as far as its last closed markdown block — the rest is still
    /// changing shape, so it stays in the viewport where it can be redrawn.
    /// Whatever this returns is appended to `history`, so a resize can replay
    /// it, and is never drawn by Fleety again.
    pub fn take_emissions(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for block in std::mem::take(&mut self.outbox) {
            self.history.push_str(&block);
            out.push(block);
        }
        while self.emitted_upto < self.messages.len() {
            let index = self.emitted_upto;
            let (role, text) = &self.messages[index];
            let in_flight = self.streaming && index + 1 == self.messages.len();

            let (body, consumed_whole) = if in_flight {
                let settled = char_boundary(text, crate::markdown::settled_prefix_len(text));
                let from = char_boundary(text, self.stream_emitted_bytes);
                if settled <= from {
                    break;
                }
                (text[from..settled].to_string(), false)
            } else {
                let from = char_boundary(text, self.stream_emitted_bytes);
                (text[from..].to_string(), true)
            };

            // The role label belongs to the first piece of a message only; a
            // reply handed over in several pieces must not repeat it.
            let label = if self.stream_emitted_bytes == 0 {
                Some(role.as_str())
            } else {
                None
            };
            let block = render_emission(role, label, &body);

            if consumed_whole {
                self.emitted_upto = index + 1;
                self.stream_emitted_bytes = 0;
            } else {
                self.stream_emitted_bytes += body.len();
            }
            if !block.is_empty() {
                self.history.push_str(&block);
                out.push(block);
            }
        }
        out
    }

    /// Everything handed to the terminal so far, for replaying after a resize.
    pub fn history(&self) -> &str {
        &self.history
    }

    /// Commit a prepared user message after the transport accepts it. Until
    /// this point the composer, attachments, transcript, and turn state stay
    /// untouched so a failed write cannot destroy unsent work.
    pub fn commit_send(&mut self) {
        self.commit_send_with_id(uuid::Uuid::new_v4().to_string());
    }

    pub fn commit_send_with_id(&mut self, message_id: String) {
        let text = take_input(&mut self.input);
        self.pending_user_confirmations
            .push_back(PendingUserConfirmation {
                message_id,
                text: text.clone(),
            });
        let attachments = std::mem::take(&mut self.pending_attachments);
        let display = if attachments.is_empty() {
            text
        } else {
            format!("{text} [+{} attachment(s)]", attachments.len())
        };
        self.push("you", display);
        self.turn_in_flight = true;
    }

    /// Commit a prepared approval decision after the transport accepts it.
    pub fn commit_approval(&mut self, approval_id: &str, approved: bool) {
        let Some((pending_id, _)) = self.pending_approvals.front() else {
            return;
        };
        if pending_id != approval_id {
            return;
        }
        if let Some((_, desc)) = self.pending_approvals.pop_front() {
            self.status = if approved {
                format!("approved: {desc}")
            } else {
                format!("denied: {desc}")
            };
        }
    }

    /// Expire approvals owned by a transport that has disconnected. The
    /// Server denies its approval gate on EOF, so carrying those UUIDs onto a
    /// replacement connection would let a successful socket write masquerade
    /// as an accepted decision.
    pub fn expire_pending_approvals(&mut self) {
        let count = self.pending_approvals.len();
        if count == 0 {
            return;
        }
        self.pending_approvals.clear();
        let message = if count == 1 {
            "approval expired after connection loss; retry the turn".to_string()
        } else {
            format!("{count} approvals expired after connection loss; retry the turn")
        };
        self.push("system", message.clone());
        self.status = message;
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

/// The largest offset at or below `at` that `text` can be sliced on.
///
/// Emission works in byte offsets, and this TUI is not allowed to panic. The
/// markdown checkpoint should already land on a character boundary, so this
/// normally returns `at` unchanged — it exists so that "should" is not what
/// stands between a multi-byte reply and a crash.
fn char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The wordmark and session facts, written once when Chat starts.
///
/// It goes into the scrollback rather than the viewport because it never
/// changes: redrawing a constant on every frame is the alternate-screen habit
/// this mode exists to drop. It scrolls away as the conversation grows, which
/// is what you want from a banner.
pub fn banner(version: &str, endpoint: &str, model: Option<&str>) -> String {
    const WORDMARK: [&str; 5] = [
        "███████ ██      ███████ ███████ ████████ ██    ██",
        "██      ██      ██      ██         ██     ██  ██ ",
        "█████   ██      █████   █████      ██      ████  ",
        "██      ██      ██      ██         ██       ██   ",
        "██      ███████ ███████ ███████    ██       ██   ",
    ];
    // Cyan wordmark, dim facts — the same two weights the chat palette uses.
    const CYAN: &str = "\x1b[36m";
    const DIM: &str = "\x1b[2m";
    const OFF: &str = "\x1b[0m";

    let mut out = String::from("\n");
    for row in WORDMARK {
        out.push_str(&format!("  {CYAN}{row}{OFF}\n"));
    }
    out.push('\n');
    out.push_str(&format!("  {DIM}v{version}  ·  {endpoint}{OFF}\n"));
    out.push_str(&format!(
        "  {DIM}model {}  ·  Enter sends  ·  Esc quits{OFF}\n\n",
        model.unwrap_or("unset")
    ));
    out
}

/// Format one piece of a message for the terminal.
///
/// Assistant replies go through markdown; everything else is the user's own
/// text and is passed through untouched.
fn render_emission(role: &str, label: Option<&str>, body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let rendered = if role == "fleety" {
        crate::markdown::render_ansi(body)
    } else {
        body.to_string()
    };
    let mut out = match label {
        Some(label) => format!("{label}: {rendered}"),
        None => rendered,
    };
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Drain the composer, returning what was in it. `TextArea` has no `take`, and
/// clearing it via `set_text("")` also resets selection and scroll — which is
/// what we want when the draft has just been sent or staged.
fn take_input(input: &mut TextArea) -> String {
    let text = input.text().to_string();
    input.set_text("");
    input.clear_history();
    text
}

/// Replace the composer's contents and park the caret at the end.
///
/// `TextArea::set_text` keeps the caret where it was, which is wrong for every
/// prefill we do — restoring a rejected draft, or seeding one in a test — where
/// the next keystroke must continue the text rather than land in front of it.
pub(crate) fn prefill(input: &mut TextArea, text: &str) {
    input.set_text(text);
    input.set_cursor(text.len());
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
    // A pending approval is modal: only y / n / Esc act until it's answered
    // (the server's gate ignores other client messages while waiting anyway).
    if !app.pending_approvals.is_empty() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                if let Some((id, _)) = app.pending_approvals.front() {
                    return Action::Approve(id.clone());
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                if let Some((id, _)) = app.pending_approvals.front() {
                    return Action::Deny(id.clone());
                }
            }
            _ => {}
        }
        return Action::None;
    }
    // Ctrl-prefixed shortcuts come before plain Char fallthrough so it doesn't
    // swallow them (e.g. Ctrl+V should NOT type 'v').
    // Two Ctrl chords mean something to Fleety that they do not mean to the
    // composer, so they are claimed before it sees them. Everything else Ctrl
    // (word motion, kill/yank, undo, the Ctrl+J newline) is the composer's.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            match c.to_ascii_lowercase() {
                'v' => return Action::PasteFromClipboard,
                'x' => {
                    app.clear_attachments();
                    return Action::None;
                }
                _ => {}
            }
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
                app.input.insert_str("\n");
                return Action::None;
            }
            let text = app.input.text().to_string();
            // `/attach <path>` stages a local file instead of sending a message.
            if let Some(path) = attach_command_path(&text) {
                if path.is_empty() {
                    app.status = "usage: /attach <path>".to_string();
                    prefill(&mut app.input, &text);
                } else if let Some(att) = crate::clipboard::attach_path(path) {
                    take_input(&mut app.input);
                    app.attach(att);
                } else {
                    // `attach_path` returns `None` for a missing/unreadable path
                    // *or* a file past the size limit — cover both so an oversized
                    // file is not misreported as "no such file".
                    app.status = format!(
                        "could not attach '{path}' — no such file, or larger than the {} MiB limit",
                        crate::clipboard::MAX_ATTACHMENT_BYTES / (1024 * 1024)
                    );
                    prefill(&mut app.input, &text);
                }
                return Action::None;
            }
            let attachments = app.pending_attachments.clone();
            if text.trim().is_empty() && attachments.is_empty() {
                Action::None
            } else {
                Action::Send { text, attachments }
            }
        }
        // Editing, cursor motion, word/line kills, yank and undo all belong to
        // the composer; it owns the key map so the two cannot drift apart.
        _ => {
            app.input.input(key);
            Action::None
        }
    }
}

/// Maximum inner rows the input box grows to before it scrolls internally.
const MAX_INPUT_ROWS: u16 = 6;

/// Draw the three panes.
#[cfg(test)]
pub fn render(frame: &mut Frame, app: &App) {
    render_in_area(frame, app, frame.area());
}

pub fn render_conversations_in_area(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::from(crate::terminal_safe_field(
        &app.conversations_status,
    ))];
    if app.conversations.is_empty() {
        lines.push(Line::from("No conversations found."));
    } else {
        lines.extend(app.conversations.iter().enumerate().map(|(index, item)| {
            let marker = if index == app.conversation_selected {
                "▶"
            } else {
                " "
            };
            let preview = if item.preview.is_empty() {
                "(no preview)".to_string()
            } else {
                crate::terminal_safe_field(&item.preview)
            };
            Line::from(format!(
                "{marker} {preview} · {} events · {}",
                item.events,
                crate::terminal_safe_field(&item.conversation_id)
            ))
        }));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Conversations"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Render Chat inside the shared workspace content region.
pub fn render_in_area(frame: &mut Frame, app: &App, area: Rect) {
    // Top to bottom: what you are talking to, then the part of the reply still
    // arriving, then the box you type in. The composer is last because that is
    // where the cursor lives — nothing should sit between it and the edge of
    // the screen. The finished conversation is above the viewport entirely; it
    // belongs to the terminal.
    let input_inner_w = area.width.saturating_sub(2);
    let input_rows = app
        .input
        .desired_height(input_inner_w)
        .clamp(1, MAX_INPUT_ROWS);
    let tail = app.viewport_tail();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(input_rows + 2),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(app.status.as_str()), chunks[0]);

    if !tail.is_empty() && chunks[1].height > 0 {
        // Show the end of the tail: it is the part still being written, and it
        // is what the reader is waiting on.
        let para = Paragraph::new(tail).wrap(Wrap { trim: false });
        let total = u16::try_from(para.line_count(chunks[1].width)).unwrap_or(u16::MAX);
        let offset = total.saturating_sub(chunks[1].height);
        frame.render_widget(para.scroll((offset, 0)), chunks[1]);
    }

    // The composer wraps and scrolls itself and keeps the cursor in view, so
    // the scroll state is rebuilt each frame from the cursor alone.
    let block = Block::bordered().title(app.input_title());
    let text_area = block.inner(chunks[2]);
    frame.render_widget(block, chunks[2]);
    let mut input_state = TextAreaState::default();
    frame.render_stateful_widget_ref(&app.input, text_area, &mut input_state);
    if let Some((cx, cy)) = app.input.cursor_pos_with_state(text_area, input_state) {
        frame.set_cursor_position((cx, cy));
    }
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
        assert_eq!(app.input.text(), "h", "prepare must retain the draft");
        assert!(
            app.messages.is_empty(),
            "prepare must not change transcript"
        );
        assert!(
            !app.turn_in_flight,
            "prepare must not claim the turn was sent"
        );
        app.commit_send();
        assert_eq!(app.input.text(), "");
        assert_eq!(app.messages.last().map(|(r, _)| r.as_str()), Some("you"));
        assert!(app.turn_in_flight);
        app.finish_assistant("hi back".to_string());
        // Empty Enter with no attachments does nothing.
        assert_eq!(on_key(&mut app, key(KeyCode::Enter)), Action::None);
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn question_mark_is_plain_composer_text() {
        let mut app = App::new("ready");
        assert_eq!(on_key(&mut app, key(KeyCode::Char('?'))), Action::None);
        assert_eq!(app.input.text(), "?");
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
        assert_eq!(app.input.text(), "see", "prepare retains the draft");
        assert_eq!(
            app.pending_attachments.len(),
            1,
            "prepare retains attachments"
        );
        app.commit_send();
        assert!(
            app.pending_attachments.is_empty(),
            "commit clears attachments"
        );
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
    fn ctrl_x_clears_attachments_instead_of_cutting_the_draft() {
        // Ctrl+X means "cut" to the composer and "drop the attachments" to
        // Fleety. Fleety wins, and the draft must survive intact.
        let mut app = App::new("ready");
        prefill(&mut app.input, "keep me");
        app.attach(WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("AAAA".into()),
            url: None,
            name: None,
        });
        assert_eq!(on_key(&mut app, ctrl('x')), Action::None);
        assert!(app.pending_attachments.is_empty());
        assert_eq!(app.input.text(), "keep me", "draft not cut");
    }

    fn emitted(app: &mut App) -> String {
        app.take_emissions().join("")
    }

    #[test]
    fn a_streaming_cjk_reply_is_split_on_character_boundaries() {
        // Emission slices the reply by byte offset. A CJK character is three
        // bytes, so an offset landing inside one would panic — and this TUI is
        // not allowed to panic.
        let mut app = App::new("ready");
        for chunk in ["第一段文字。\n\n", "第二段還沒寫完", "的中文\n\n"] {
            app.push_delta(chunk);
            let _ = app.take_emissions();
            let _ = app.viewport_tail();
        }
        app.finish_assistant("第一段文字。\n\n第二段還沒寫完的中文\n\n".to_string());
        let out = app.take_emissions().join("");
        let all = format!("{}{out}", app.history());
        assert!(all.contains("第一段文字"), "content survives: {all:?}");
    }

    #[test]
    fn the_banner_goes_out_before_the_conversation_and_only_once() {
        let mut app = App::new("ready");
        app.announce(banner("9.9.9", "ws://127.0.0.1:9999", Some("gpt-x")));
        app.push("you", "hello");

        let out = emitted(&mut app);
        let mark = out.find("FLEETY").or_else(|| out.find('█'));
        assert!(mark.is_some(), "the wordmark is drawn: {out:?}");
        assert!(
            mark < out.find("you: hello"),
            "and it goes out ahead of the conversation"
        );
        assert!(
            out.contains("9.9.9") && out.contains("gpt-x"),
            "facts: {out:?}"
        );

        // It lives in the scrollback, so it is never drawn again — but a resize
        // has to be able to replay it.
        assert!(emitted(&mut app).is_empty(), "not re-emitted");
        assert!(app.history().contains('█'), "replayable after a resize");
    }

    #[test]
    fn a_completed_exchange_is_handed_to_the_terminal_whole() {
        let mut app = App::new("ready");
        app.push("you", "hi");
        app.push("fleety", "hello");
        let out = emitted(&mut app);
        assert!(out.contains("you: hi"), "user message handed over: {out:?}");
        assert!(out.contains("hello"), "reply handed over: {out:?}");

        // Handed over means gone from Fleety's hands: nothing is emitted twice.
        assert!(
            emitted(&mut app).is_empty(),
            "an emitted message is never handed over again"
        );
        assert!(
            app.viewport_tail().is_empty(),
            "nothing left in the viewport"
        );
    }

    #[test]
    fn a_multi_line_message_keeps_its_breaks_when_handed_over() {
        let mut app = App::new("ready");
        app.push("fleety", "first line\nsecond line");
        let out = emitted(&mut app);
        assert!(out.contains("first line"), "{out:?}");
        assert!(out.contains("second line"), "{out:?}");
        assert!(
            !out.contains("first linesecond"),
            "the break survives: {out:?}"
        );
    }

    #[test]
    fn history_accumulates_everything_handed_over() {
        let mut app = App::new("ready");
        app.push("you", "one");
        let first = emitted(&mut app);
        app.push("you", "two");
        let second = emitted(&mut app);
        assert_eq!(
            app.history(),
            format!("{first}{second}"),
            "history is the replay source for a resize, so it must be exact"
        );
    }

    #[test]
    fn the_viewport_holds_only_the_composer_and_status_when_nothing_is_in_flight() {
        let mut app = App::new("ready");
        app.push("you", "hi");
        app.push("fleety", "hello");
        let _ = app.take_emissions();

        let mut terminal = Terminal::new(TestBackend::new(40, 6)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let rows = visible_rows(&terminal);
        assert!(
            rows.iter().any(|r| r.contains("Message")),
            "composer drawn: {rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("ready")),
            "status drawn: {rows:#?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("hello")),
            "the conversation is the terminal's, not drawn here: {rows:#?}"
        );
    }

    #[test]
    fn the_composer_fits_the_viewport_height_that_was_asked_for() {
        // Regression: the height was computed for the content alone while the
        // workspace chrome drew a header line above it, so the composer was
        // squeezed out of the frame entirely — and every existing test missed
        // it by calling `render_in_area` directly instead of going through the
        // chrome the real loop uses.
        let app = App::new("ready");
        let width = 60;
        let rows = app.viewport_height(width, 15) + crate::workspace::INLINE_CHROME_ROWS;

        let mut terminal = Terminal::new(TestBackend::new(width, rows)).expect("term");
        let state = crate::workspace::WorkspaceState::new(crate::workspace::Route::Chat);
        terminal
            .draw(|f| {
                crate::workspace::render_inline(f, &state, |f, area| render_in_area(f, &app, area))
            })
            .expect("draw");
        let rows_text = visible_rows(&terminal);
        let top = rows_text
            .iter()
            .position(|r| r.contains('┌'))
            .expect("composer top border");
        let bottom = rows_text
            .iter()
            .position(|r| r.contains('└'))
            .expect("composer bottom border");
        assert!(
            bottom > top + 1,
            "the composer has a row to type into. Sized one row short it still \
             draws both borders with nothing between them, which looks fine and \
             cannot be typed in: {rows_text:#?}"
        );
        assert!(
            rows_text.iter().any(|r| r.contains("ready")),
            "the status line is drawn: {rows_text:#?}"
        );
    }

    #[test]
    fn a_streaming_reply_shows_its_end_in_the_viewport() {
        let mut app = App::new("ready");
        for i in 0..40 {
            app.push_delta(&format!("line {i}\n"));
        }
        let _ = app.take_emissions();

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let rows = visible_rows(&terminal);
        assert!(
            rows.iter().any(|r| r.contains("line 39")),
            "the newest content is what the reader is waiting on: {rows:#?}"
        );
    }

    /// Drop ANSI escape sequences, leaving the text a reader would see.
    fn without_escapes(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c != '\u{1b}' {
                out.push(c);
                continue;
            }
            // CSI/SGR run: skip up to and including the final byte.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
        out
    }

    #[test]
    fn a_handed_over_code_block_carries_its_styling() {
        let mut app = App::new("ready");
        app.push(
            "fleety",
            "intro\n```rust\nfn main() { println!(\"hi\"); }\n```",
        );
        let out = app.take_emissions().join("");
        assert!(
            without_escapes(&out).contains("fn main()"),
            "code kept: {out:?}"
        );
        // Syntect colours the code, and that styling has to travel with the
        // text as escape sequences — the terminal owns these rows now, so there
        // is no ratatui buffer left to carry a Style. The exact SGR form follows
        // terminal capability and may be ANSI-16, ANSI-256, or truecolor.
        let code = out
            .split_once("rust\u{1b}[0m\n")
            .map(|(_, code)| code)
            .unwrap_or_default()
            .split("\u{1b}[0m\u{1b}[2m```")
            .next()
            .unwrap_or_default();
        let styled = code
            .split("\u{1b}[")
            .skip(1)
            .any(|sgr| !sgr.starts_with("0m"));
        if fleety_markdown::get_color_level() == fleety_markdown::ColorLevel::None {
            assert!(
                !styled,
                "NO_COLOR must suppress syntax colours in scrollback: {out:?}"
            );
        } else {
            assert!(styled, "syntax colours travel with the text: {out:?}");
        }
    }

    #[test]
    fn long_line_wraps_in_the_composer_instead_of_scrolling_sideways() {
        // The old editor kept one long line on one row and scrolled it
        // horizontally, so the start of what you typed went off-screen. The
        // composer wraps instead, and the box grows a row to hold the wrap.
        let mut app = App::new("ready");
        let typed = "the quick brown fox jumps over the lazy dog";
        prefill(&mut app.input, typed);
        let mut terminal = Terminal::new(TestBackend::new(30, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let rows = visible_rows(&terminal);

        let head = rows.iter().position(|r| r.contains("the quick brown"));
        let tail = rows.iter().position(|r| r.contains("lazy dog"));
        assert!(
            head.is_some(),
            "start of the line still visible:\n{rows:#?}"
        );
        assert!(tail.is_some(), "end of the line also visible:\n{rows:#?}");
        assert_ne!(head, tail, "the line wrapped onto a second row");
    }

    #[test]
    fn composer_owns_word_kill_and_undo() {
        // Everything Fleety does not claim reaches the composer's own key map.
        // These two are the ones the old editor had no answer for at all.
        let mut app = App::new("ready");
        for c in "hello world".chars() {
            on_key(&mut app, key(KeyCode::Char(c)));
        }
        assert_eq!(app.input.text(), "hello world");

        on_key(&mut app, ctrl('w'));
        assert_eq!(app.input.text(), "hello ", "Ctrl+W kills the last word");

        on_key(&mut app, ctrl('z'));
        assert_eq!(app.input.text(), "hello world", "Ctrl+Z undoes the kill");
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
        assert!(app.apply_replay("c1", 6, "user", "new line"));
        assert_eq!(app.messages.len(), before + 1);
        assert_eq!(
            app.messages.last().map(|(role, _)| role.as_str()),
            Some("you")
        );
        assert_eq!(app.last_seq, 6);
        // Replaying that same seq again is now a duplicate.
        assert!(!app.apply_replay("c1", 6, "user", "new line"));
        assert_eq!(app.messages.len(), before + 1);
    }

    #[test]
    fn replayed_user_event_reconciles_the_locally_committed_send() {
        let mut app = App::new("ready");
        app.note_seq("c1", 5);
        prefill(&mut app.input, "same message");
        app.commit_send();
        let before = app.messages.len();

        assert!(app.apply_replay("c1", 6, "user", "same message"));
        assert_eq!(
            app.messages.len(),
            before,
            "the authoritative replay must confirm, not duplicate, the local send"
        );
        assert_eq!(app.last_seq, 6);
        assert!(
            app.turn_in_flight,
            "the user replay does not finish the turn"
        );

        assert!(app.apply_replay("c1", 7, "assistant", "the answer"));
        assert_eq!(
            app.messages.last(),
            Some(&("fleety".to_string(), "the answer".to_string()))
        );
        assert!(
            !app.turn_in_flight,
            "a replayed assistant reply finishes the turn"
        );
    }

    #[test]
    fn replay_reconciles_multiple_locally_committed_interjections_in_order() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "first message");
        app.commit_send();
        prefill(&mut app.input, "second message");
        app.commit_send();
        let local_messages = app.messages.len();

        assert!(app.apply_replay("c1", 1, "user", "first message"));
        assert!(app.apply_replay("c1", 2, "assistant", "first answer"));
        assert!(app.apply_replay("c1", 3, "user", "second message"));

        assert_eq!(
            app.messages.len(),
            local_messages + 1,
            "both local user messages are confirmed without duplicate transcript lines"
        );
        assert_eq!(
            app.messages.last(),
            Some(&("fleety".to_string(), "first answer".to_string()))
        );
    }

    #[test]
    fn failed_turn_keeps_its_user_confirmation_until_replay() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "stored before provider failure");
        app.commit_send();
        app.finish_error();
        let local_messages = app.messages.len();

        assert!(app.apply_replay("c1", 1, "user", "stored before provider failure"));
        assert_eq!(
            app.messages.len(),
            local_messages,
            "a later reconnect confirms the failed turn without duplicating it"
        );
    }

    #[test]
    fn successful_queued_turn_retires_the_failed_and_successful_confirmations() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "first message");
        app.commit_send();
        prefill(&mut app.input, "queued message");
        app.commit_send();

        app.finish_error();
        app.finish_assistant("queued answer".to_string());

        assert!(
            app.pending_user_confirmations.is_empty(),
            "the later authoritative assistant reply covers the failed prefix and queued turn"
        );
    }

    #[test]
    fn duplicate_error_frame_does_not_fail_the_next_confirmation() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active message");
        app.commit_send();
        prefill(&mut app.input, "queued message");
        app.commit_send();

        app.finish_error();
        app.finish_error();

        assert_eq!(app.failed_user_confirmations, 1);
        assert!(app.turn_in_flight);
    }

    #[test]
    fn interjection_notice_keeps_replay_confirmation_and_turn_active() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active message");
        app.commit_send();
        prefill(&mut app.input, "queued interjection");
        app.commit_send();

        app.push_assistant_notice("got it — right after this".to_string());

        assert_eq!(app.pending_user_confirmations.len(), 2);
        assert!(app.turn_in_flight);
        let displayed = app.messages.len();
        assert!(app.apply_replay("c1", 1, "user", "active message"));
        assert_eq!(
            app.messages.len(),
            displayed,
            "a reconnect after the acknowledgement must not duplicate the active message"
        );
    }

    #[test]
    fn queued_interjection_ack_keeps_both_confirmations_active() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active message");
        app.commit_send();
        prefill(&mut app.input, "queued interjection");
        app.commit_send();

        app.acknowledge_interjection(
            "unknown-but-accepted",
            InterjectionDisposition::Queued,
            "got it — right after this".to_string(),
        );

        assert_eq!(app.pending_user_confirmations.len(), 2);
        assert!(app.turn_in_flight);
    }

    #[test]
    fn ignored_interjection_ack_retires_only_the_ignored_confirmation() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active message");
        app.commit_send();
        prefill(&mut app.input, "ignore this interjection");
        app.commit_send_with_id("ignored".to_string());

        app.acknowledge_interjection(
            "ignored",
            InterjectionDisposition::Ignored,
            "noted — this message won't start another turn".to_string(),
        );

        assert_eq!(
            app.pending_user_confirmations
                .iter()
                .map(|confirmation| confirmation.text.as_str())
                .collect::<Vec<_>>(),
            ["active message"]
        );
        assert!(app.turn_in_flight);
    }

    #[test]
    fn rejected_interjection_ack_retires_only_the_rejected_confirmation() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active message");
        app.commit_send();
        prefill(&mut app.input, "overflow message");
        app.commit_send_with_id("rejected".to_string());

        app.acknowledge_interjection(
            "rejected",
            InterjectionDisposition::Rejected,
            "the interjection queue is full".to_string(),
        );

        assert_eq!(
            app.pending_user_confirmations
                .iter()
                .map(|confirmation| confirmation.text.as_str())
                .collect::<Vec<_>>(),
            ["active message"]
        );
        assert!(app.turn_in_flight);
    }

    #[test]
    fn interjection_ack_retires_the_matching_confirmation_not_the_newest() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active message");
        app.commit_send_with_id("active".to_string());
        prefill(&mut app.input, "first interjection");
        app.commit_send_with_id("first".to_string());
        prefill(&mut app.input, "second interjection");
        app.commit_send_with_id("second".to_string());

        app.acknowledge_interjection(
            "first",
            InterjectionDisposition::Ignored,
            "noted — this message won't start another turn".to_string(),
        );

        let pending: Vec<&str> = app
            .pending_user_confirmations
            .iter()
            .map(|confirmation| confirmation.text.as_str())
            .collect();
        assert_eq!(pending, ["active message", "second interjection"]);
        assert!(app.turn_in_flight);
    }

    #[test]
    fn done_makes_a_seq_zero_terminal_reply_finish_the_turn() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "forbidden conversation");
        app.commit_send();

        app.push_assistant_notice("That conversation isn't available to you.".to_string());
        app.finish_done();

        assert!(!app.turn_in_flight);
        assert!(app.pending_user_confirmations.is_empty());
    }

    #[test]
    fn reconnect_clears_prior_error_before_a_seq_zero_terminal_turn() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "failed before cleanup");
        app.commit_send();
        app.finish_error();

        app.prepare_for_reconnect();
        prefill(&mut app.input, "blocked after reconnect");
        app.commit_send();
        app.push_assistant_notice("That request is not available.".to_string());
        app.finish_done();

        assert!(!app.turn_in_flight);
        assert!(app.pending_user_confirmations.is_empty());
        assert_eq!(app.failed_user_confirmations, 0);
    }

    #[test]
    fn resume_done_does_not_retire_a_fast_post_reconnect_send() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active when transport dropped");
        app.commit_send();
        app.prepare_for_reconnect();
        app.begin_resume();

        prefill(&mut app.input, "sent before resume done");
        app.commit_send();
        assert!(app.apply_replay("c1", 1, "user", "active when transport dropped"));
        assert!(app.apply_replay("c1", 2, "assistant", "recovered prior turn"));
        app.finish_done();

        assert!(app.turn_in_flight);
        assert_eq!(app.pending_user_confirmations.len(), 1);
        assert_eq!(app.failed_user_confirmations, 0);

        app.finish_assistant("new turn answer".to_string());
        app.finish_done();
        assert!(!app.turn_in_flight);
        assert!(app.pending_user_confirmations.is_empty());
    }

    #[test]
    fn queued_confirmation_stays_in_flight_between_turn_done_frames() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "active message");
        app.commit_send();
        prefill(&mut app.input, "queued interjection");
        app.commit_send();

        app.finish_assistant("first answer".to_string());
        app.finish_done();
        assert!(app.turn_in_flight);
        assert_eq!(app.pending_user_confirmations.len(), 1);

        app.finish_assistant("queued answer".to_string());
        app.finish_done();
        assert!(!app.turn_in_flight);
        assert!(app.pending_user_confirmations.is_empty());
    }

    #[test]
    fn a_different_replayed_user_event_is_preserved() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "local message");
        app.commit_send();

        assert!(app.apply_replay("c1", 1, "user", "message from elsewhere"));
        assert_eq!(
            app.messages.last(),
            Some(&("you".to_string(), "message from elsewhere".to_string()))
        );
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

    /// One terminal row per rendered line, for asserting what's visible where.
    fn visible_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buf = terminal.backend().buffer();
        let w = buf.area.width as usize;
        let syms: Vec<&str> = buf.content().iter().map(|c| c.symbol()).collect();
        syms.chunks(w).map(|r| r.concat()).collect()
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
        prefill(&mut app.input, "aaa\nbbb");
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
        prefill(
            &mut app.input,
            &format!("/attach {}", path.to_string_lossy()),
        );
        assert_eq!(on_key(&mut app, key(KeyCode::Enter)), Action::None);
        assert_eq!(app.pending_attachments.len(), 1);
        assert!(
            app.input.is_empty(),
            "input cleared after a successful attach"
        );

        // A missing path preserves the input and reports an error — nothing new
        // is staged and no message is sent.
        let missing = "/attach /no/such/file-xyz".to_string();
        prefill(&mut app.input, &missing);
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
        prefill(&mut app.input, "hello");
        let Action::Send { .. } = on_key(&mut app, key(KeyCode::Enter)) else {
            panic!("expected Send");
        };
        app.commit_send();
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
        prefill(&mut app.input, "draft");
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
        prefill(&mut app.input, "draft");
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
        prefill(&mut app.input, "draft");
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
        prefill(&mut app.input, "go");
        let _ = on_key(&mut app, key(KeyCode::Enter));
        app.commit_send();
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
        assert_eq!(app.pending_approvals.len(), 1, "prepare retains approval");
        app.commit_approval("id1", true);
        assert!(app.pending_approvals.is_empty());
        // Esc on a pending approval denies — it does not quit.
        app.request_approval("id2".into(), "run_command", "critical", "rm x");
        assert_eq!(
            on_key(&mut app, key(KeyCode::Esc)),
            Action::Deny("id2".into())
        );
        assert_eq!(app.pending_approvals.len(), 1, "prepare retains denial");
        app.commit_approval("id2", false);
        assert!(!app.should_quit);
        // With no approval pending, Esc quits as before.
        assert_eq!(on_key(&mut app, key(KeyCode::Esc)), Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn expired_transport_approval_cannot_be_committed_and_preserves_composer() {
        let mut app = App::new("ready");
        prefill(&mut app.input, "keep this draft");
        app.attach(WireAttachment {
            mime: "text/plain".into(),
            bytes_b64: Some("aGVsbG8=".into()),
            url: None,
            name: Some("note.txt".into()),
        });
        app.request_approval("old-id".into(), "write_file", "mutate", "write foo");

        app.expire_pending_approvals();

        assert!(app.pending_approvals.is_empty());
        assert_eq!(app.input.text(), "keep this draft");
        assert_eq!(on_key(&mut app, key(KeyCode::Char('y'))), Action::None);
        app.commit_approval("old-id", true);
        assert!(!app.status.contains("approved"));
        assert!(app.status.contains("expired"));
        assert_eq!(app.input.text(), "keep this drafty");
        assert_eq!(app.pending_attachments.len(), 1);
    }

    #[test]
    fn conversation_list_parses_and_renders_real_server_rows() {
        let mut app = App::new("ready");
        app.conversations = parse_conversation_summaries(
            r#"[{"conversation_id":"c-1","last_ts_secs":123,"events":4,"preview":"部署進度 🚀"}]"#,
        )
        .expect("parse conversation list");
        assert_eq!(app.conversations[0].preview, "部署進度 🚀");
        app.conversations_status = "1 conversation(s)".into();
        let backend = ratatui::backend::TestBackend::new(80, 12);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_conversations_in_area(frame, &app, frame.area()))
            .expect("render conversations");
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(content.contains('🚀'), "{content}");
        assert!(content.contains("4 events"), "{content}");
        assert!(content.contains("c-1"), "{content}");
        assert!(!content.contains('�'), "{content}");
    }
}
