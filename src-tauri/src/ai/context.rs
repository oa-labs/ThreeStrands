//! Bounds the thread content sent to a provider: which messages, how much of
//! each body, and quoted history the model already has.

/// One thread message reduced to what the summarizer needs, oldest first.
pub struct ThreadMessageInput {
    pub sender: String,
    pub sent_at: String,
    pub body_text: String,
}

/// One bounded message supplied to explicit thread-action extraction. The
/// message id is retained so every proposal can point back to verifiable
/// evidence without giving the model access to any application tools.
pub struct ActionMessageInput {
    pub id: String,
    pub sender: String,
    /// The message's To addresses, so the model can tell a request made of
    /// the user from one made of someone else while the user is copied.
    pub recipients: Vec<String>,
    pub sent_at: String,
    pub body_text: String,
}

pub(super) fn is_new_message(sent_at: &str, focus_from: Option<&str>) -> bool {
    focus_from.is_none_or(|focus_from| sent_at >= focus_from)
}

/// Bounds the prompt to a handful of recent messages, and each message to a
/// reasonable length, so a long thread doesn't blow past a provider's context
/// window or run up an outsized bill for a shortcut meant to save a click.
pub(super) const MAX_MESSAGES: usize = 15;
pub(super) const MAX_BODY_CHARS: usize = 10_000;

/// Earlier messages are background for the new ones, so each sends less.
pub(super) const MAX_BACKGROUND_BODY_CHARS: usize = 2_000;

pub(super) fn action_context(messages: &[ActionMessageInput], focus_from: Option<&str>) -> Vec<ActionMessageInput> {
    let window = &messages[messages.len().saturating_sub(MAX_MESSAGES)..];
    let bodies = bounded_bodies_capped(window.iter().map(|message| {
        let cap = if is_new_message(&message.sent_at, focus_from) { MAX_BODY_CHARS } else { MAX_BACKGROUND_BODY_CHARS };
        (message.body_text.as_str(), cap)
    }));
    window
        .iter()
        .zip(bodies)
        .map(|(message, body_text)| ActionMessageInput {
            id: message.id.clone(),
            sender: message.sender.clone(),
            recipients: message.recipients.clone(),
            sent_at: message.sent_at.clone(),
            body_text,
        })
        .collect()
}

pub(super) fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// Bodies for a window of thread messages, oldest first, exactly as the
/// provider receives them. Quoted history is dropped only where it repeats
/// text that an earlier body in this same window already sends — after that
/// body's own capping — so the model never loses content it would otherwise
/// have seen. Each body is then capped at `max_chars`.
pub(super) fn bounded_bodies<'a>(bodies: impl IntoIterator<Item = &'a str>, max_chars: usize) -> Vec<String> {
    bounded_bodies_capped(bodies.into_iter().map(|body| (body, max_chars)))
}

/// `bounded_bodies` with a cap per body.
pub(super) fn bounded_bodies_capped<'a>(bodies: impl IntoIterator<Item = (&'a str, usize)>) -> Vec<String> {
    let mut history = crate::quoted_history::ThreadHistory::default();
    bodies
        .into_iter()
        .map(|(body, max_chars)| {
            let new_len = history.new_text_len(body);
            let new_text = if new_len < body.len() { body[..new_len].trim_end() } else { body };
            let sent = truncate_chars(new_text, max_chars);
            history.remember(&sent);
            sent
        })
        .collect()
}

/// How many of a thread's messages a brief, summary, or suggestion request sends.
pub fn sent_message_count(messages: usize) -> usize {
    messages.min(MAX_MESSAGES)
}
