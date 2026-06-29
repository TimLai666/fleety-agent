//! Mid-turn interjection triage.
//!
//! When a new user message arrives while a turn is running, a lightweight model
//! call classifies how to handle it: interrupt the current run, queue it for
//! after, or ignore it. Parsing the model's answer is pure (and unit-tested);
//! a failed/unparseable triage defaults conservatively to "queue after" so an
//! in-progress turn is never interrupted on a guess.

use agent_core::{Message, ModelProvider};

/// What to do with a message that arrived mid-turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageAction {
    /// Stop the current run at its next checkpoint, then handle this message.
    InterruptNow,
    /// Let the current run finish, then handle this message.
    QueueAfter,
    /// Don't act on it (acknowledge only).
    Ignore,
}

/// Map a triage model's free-text answer to an action. Conservative: anything
/// that isn't clearly "interrupt" or "ignore" is `QueueAfter`.
pub fn parse_triage(model_text: &str) -> TriageAction {
    let t = model_text.to_ascii_lowercase();
    if t.contains("interrupt") {
        TriageAction::InterruptNow
    } else if t.contains("ignore") {
        TriageAction::Ignore
    } else {
        TriageAction::QueueAfter
    }
}

/// Classify a mid-turn message with one lightweight model call. `turn_summary`
/// describes what the active turn is doing. A failed or unparseable call →
/// `QueueAfter` (never interrupt on uncertainty).
pub async fn triage(
    new_msg: &str,
    turn_summary: &str,
    provider: &dyn ModelProvider,
) -> TriageAction {
    let summary = if turn_summary.trim().is_empty() {
        "(working on the previous request)"
    } else {
        turn_summary
    };
    let prompt = format!(
        "Fleety is in the middle of a task:\n{summary}\n\nThe user just sent another message:\n\
         \"{new_msg}\"\n\nDecide how to handle the new message. Answer with exactly one word:\n\
         - interrupt_now : stop the current work now and handle this message\n\
         - queue_after   : finish the current work first, then handle it\n\
         - ignore        : it needs no action (e.g. an aside or acknowledgement)\n\
         One word only."
    );
    match provider.complete(&[Message::user(prompt)], &[]).await {
        Ok(resp) => parse_triage(resp.message.content.as_deref().unwrap_or("")),
        Err(_) => TriageAction::QueueAfter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_triage_table() {
        assert_eq!(parse_triage("interrupt_now"), TriageAction::InterruptNow);
        assert_eq!(parse_triage("INTERRUPT please"), TriageAction::InterruptNow);
        assert_eq!(parse_triage("queue_after"), TriageAction::QueueAfter);
        assert_eq!(parse_triage("ignore"), TriageAction::Ignore);
        // Unparseable / unexpected → conservative QueueAfter.
        assert_eq!(parse_triage("¯\\_(ツ)_/¯"), TriageAction::QueueAfter);
        assert_eq!(parse_triage(""), TriageAction::QueueAfter);
    }
}
