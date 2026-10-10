//! Pure, table-tested local threading — group messages into threads by their
//! `Message-ID` / `In-Reply-To` / `References`, with NO subject grouping and
//! NO network I/O.
//!
//! `docs/imap-design.md` ("Threading"). Most IMAP servers return no
//! `THREADID`, so threading is computed locally. The rules this module
//! implements, exactly:
//!
//! * Messages are grouped by message references: a message's normalized
//!   `Message-ID`, `In-Reply-To` and `References` are combined per account,
//!   and messages that point at each other share a thread. This is the
//!   reference part of the JWZ algorithm — the subject-grouping pass is
//!   deliberately omitted, because a wrong merge of two unrelated "Invoice"
//!   threads is worse than a thread left split.
//! * A thread's id is fixed when the thread is created from the hash of the
//!   first message id it saw.
//! * A late message can link two existing threads. The OLDER thread's id
//!   survives, the other's messages are re-pointed to it, an alias row is
//!   written old -> survivor, and BOTH ids are reported changed so the sync
//!   engine re-ingests them.
//!
//! "Older" is decided by thread-creation order, which the caller supplies as a
//! monotonic sequence (lower = created earlier). This is deterministic and
//! needs no clock: the first message id a thread ever saw also fixes its id,
//! so two runs over the same input in the same order thread identically.
//!
//! All header values are sender-controlled, so nothing here panics: the one
//! shared [`normalize_message_id`](super::identity) is reused for every id,
//! and the `References` chain is bounded by
//! [`policy::clamp_references`](super::policy) before it is walked, so a
//! hostile multi-thousand-id header cannot force quadratic work. Over-cap
//! references are reported, never silently dropped.

use std::collections::BTreeMap;

use super::identity::normalize_message_id;
use super::policy;

/// Hash a normalized message id into the thread-id body. A local, panic-free
/// hex SHA-256, mirroring `identity`'s own hashing so threading pulls in no
/// new dependency.
fn thread_hash(normalized_message_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"imap-thread\n");
    hasher.update(normalized_message_id.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// The thread id a brand-new thread takes, fixed from the first (already
/// normalized) message token it saw: `imap:<account>:t:<hash>`.
pub fn new_thread_id(account: &str, first_message_id: &str) -> String {
    format!(
        "imap:{account}:t:{}",
        thread_hash(&normalize_message_id(first_message_id))
    )
}

/// One message handed to the threader: its already-derived stable id, and the
/// raw reference headers as the identity pass collected them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadingInput {
    /// The stable cross-account message id (from `identity::derive_message_id`).
    pub message_id: String,
    /// The raw `Message-ID` header value, if present.
    pub message_id_header: Option<String>,
    /// The raw `In-Reply-To` header value, if present.
    pub in_reply_to: Option<String>,
    /// The raw `References` header, space-separated ids, if present.
    pub references: Option<String>,
}

impl ThreadingInput {
    /// The normalized reference tokens this message links through, nearest
    /// ancestor last, capped by [`policy::clamp_references`]. Its own
    /// `Message-ID` is excluded here (keyed separately); these are the OTHER
    /// messages it points at. Returns the kept tokens and how many the cap
    /// dropped.
    fn reference_tokens(&self) -> (Vec<String>, usize) {
        let mut raw: Vec<String> = Vec::new();
        if let Some(references) = self.references.as_deref() {
            for token in references.split_whitespace() {
                let normalized = normalize_message_id(token);
                if !normalized.is_empty() {
                    raw.push(normalized);
                }
            }
        }
        // In-Reply-To is the nearest ancestor; append last so the cap keeps it.
        if let Some(in_reply_to) = self.in_reply_to.as_deref() {
            let normalized = normalize_message_id(in_reply_to);
            if !normalized.is_empty() && raw.last() != Some(&normalized) {
                raw.push(normalized);
            }
        }
        let selection = policy::clamp_references(raw);
        (selection.kept, selection.dropped)
    }

    /// This message's own normalized Message-ID token. Falls back to the
    /// stable id when the header is absent so a message with no `Message-ID`
    /// still anchors its own singleton thread without colliding with another.
    fn own_token(&self) -> String {
        self.message_id_header
            .as_deref()
            .map(normalize_message_id)
            .filter(|token| !token.is_empty())
            .unwrap_or_else(|| self.message_id.clone())
    }
}

/// The outcome of threading one batch of messages against the existing state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadingOutcome {
    /// Final `(message_id -> thread_id)` assignment for every input message.
    pub assignments: Vec<(String, String)>,
    /// Merge aliases produced this batch: `(old_thread_id, surviving_id)`.
    pub aliases: Vec<(String, String)>,
    /// Every thread id touched this batch (new, extended, or merged away),
    /// so the caller journals them. De-duplicated, sorted.
    pub changed_threads: Vec<String>,
    /// How many reference ids were dropped by the cap across the batch, so the
    /// caller can record the chain was shortened (never silent).
    pub references_dropped: usize,
}

/// The existing thread state the threader reads and updates: which thread each
/// known normalized reference token belongs to, and each thread's creation
/// order. The caller rebuilds it from the store so this module stays pure.
#[derive(Clone, Debug, Default)]
pub struct ThreadState {
    /// `normalized token -> thread_id` for every token already seen.
    token_thread: BTreeMap<String, String>,
    /// `thread_id -> creation sequence` (lower = older).
    created_seq: BTreeMap<String, u64>,
    /// Next creation sequence to hand out.
    next_seq: u64,
}

impl ThreadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a pre-existing thread: its id, its creation sequence, and the
    /// tokens already known to belong to it. Tokens must be in the SAME shape
    /// `thread_batch` compares against: a normalized `Message-ID` token (as
    /// [`normalize_message_id`] produces) or a stable message id used verbatim.
    /// Lets the sync routine (and tests) rebuild prior state before threading a
    /// new batch.
    pub fn seed(&mut self, thread_id: &str, created_seq: u64, tokens: &[String]) {
        self.created_seq.insert(thread_id.to_string(), created_seq);
        self.next_seq = self.next_seq.max(created_seq + 1);
        for token in tokens {
            self.token_thread.insert(token.clone(), thread_id.to_string());
        }
    }

    fn allocate_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }
}

/// Thread one batch of messages into `state`, returning the assignments,
/// merges and changed ids. Processes messages in the given order; order
/// decides which of two linked threads is "older" only when both were created
/// within this batch (a seeded thread already has its sequence).
///
/// Pure: mutates only the passed-in `state`, performs no I/O.
pub fn thread_batch(state: &mut ThreadState, messages: &[ThreadingInput]) -> ThreadingOutcome {
    let mut outcome = ThreadingOutcome::default();
    let mut changed: BTreeMap<String, ()> = BTreeMap::new();

    for message in messages {
        let own = message.own_token();
        let (refs, dropped) = message.reference_tokens();
        outcome.references_dropped += dropped;

        // Every token this message ties together: its own id plus the ones it
        // references.
        let mut tokens: Vec<String> = Vec::with_capacity(refs.len() + 1);
        tokens.push(own.clone());
        for token in &refs {
            if !tokens.contains(token) {
                tokens.push(token.clone());
            }
        }

        // Which existing threads do these tokens touch?
        let mut touched: Vec<String> = Vec::new();
        for token in &tokens {
            if let Some(thread) = state.token_thread.get(token) {
                if !touched.contains(thread) {
                    touched.push(thread.clone());
                }
            }
        }

        let thread_id = match touched.as_slice() {
            // No existing thread: create one, id fixed from this message's own
            // token (the first id the thread saw).
            [] => {
                let id = new_thread_id_from_token(&own);
                let seq = state.allocate_seq();
                state.created_seq.insert(id.clone(), seq);
                id
            }
            // Exactly one existing thread: join it.
            [existing] => existing.clone(),
            // Several threads linked by this message: MERGE. The oldest
            // (lowest creation sequence) survives; the rest alias to it.
            _ => merge_threads(state, &touched, &mut outcome, &mut changed),
        };

        // Point every token (own + refs) at the chosen thread.
        for token in &tokens {
            state.token_thread.insert(token.clone(), thread_id.clone());
        }
        outcome
            .assignments
            .push((message.message_id.clone(), thread_id.clone()));
        changed.insert(thread_id.clone(), ());
    }

    outcome.changed_threads = changed.into_keys().collect();
    outcome
}

/// The thread id for a brand-new thread, derived from the first token it saw.
/// The token is already normalized. The hash alone distinguishes threads; the
/// `imap:t:` prefix keeps it a readable, collision-resistant id consistent
/// with [`new_thread_id`].
fn new_thread_id_from_token(normalized_token: &str) -> String {
    format!("imap:t:{}", thread_hash(normalized_token))
}

/// Test-only: the thread id a batch-created thread takes for a given RAW
/// token, so a test can predict the id and its hash ordering. Normalizes the
/// token the same way `thread_batch` does.
#[cfg(test)]
pub(super) fn new_thread_id_for_test(raw_token: &str) -> String {
    new_thread_id_from_token(&normalize_message_id(raw_token))
}

/// Collapse several threads into their oldest member, writing aliases for the
/// rest and re-pointing every token that pointed at a merged-away thread.
fn merge_threads(
    state: &mut ThreadState,
    touched: &[String],
    outcome: &mut ThreadingOutcome,
    changed: &mut BTreeMap<String, ()>,
) -> String {
    // Survivor: lowest creation sequence (oldest). Ties break on the id for
    // determinism; an unknown sequence sorts last.
    let survivor = touched
        .iter()
        .min_by(|a, b| {
            let sa = state.created_seq.get(*a).copied().unwrap_or(u64::MAX);
            let sb = state.created_seq.get(*b).copied().unwrap_or(u64::MAX);
            sa.cmp(&sb).then_with(|| a.cmp(b))
        })
        .cloned()
        .unwrap_or_default();

    for merged in touched {
        if merged == &survivor {
            continue;
        }
        for thread in state.token_thread.values_mut() {
            if thread == merged {
                *thread = survivor.clone();
            }
        }
        outcome.aliases.push((merged.clone(), survivor.clone()));
        changed.insert(merged.clone(), ());
    }
    changed.insert(survivor.clone(), ());
    survivor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        id: &str,
        msgid: &str,
        in_reply_to: Option<&str>,
        references: Option<&str>,
    ) -> ThreadingInput {
        ThreadingInput {
            message_id: id.to_string(),
            message_id_header: Some(msgid.to_string()),
            in_reply_to: in_reply_to.map(str::to_string),
            references: references.map(str::to_string),
        }
    }

    fn thread_of<'a>(outcome: &'a ThreadingOutcome, message_id: &str) -> &'a str {
        outcome
            .assignments
            .iter()
            .find(|(id, _)| id == message_id)
            .map(|(_, thread)| thread.as_str())
            .expect("message was assigned a thread")
    }

    #[test]
    fn a_singleton_message_gets_its_own_thread() {
        let mut state = ThreadState::new();
        let outcome = thread_batch(&mut state, &[input("imap:a:m1", "<m1@x>", None, None)]);
        assert_eq!(outcome.assignments.len(), 1);
        assert_eq!(outcome.changed_threads.len(), 1);
        assert!(outcome.aliases.is_empty());
    }

    #[test]
    fn a_reply_joins_its_parents_thread() {
        let mut state = ThreadState::new();
        let outcome = thread_batch(
            &mut state,
            &[
                input("imap:a:m1", "<m1@x>", None, None),
                input("imap:a:m2", "<m2@x>", Some("<m1@x>"), Some("<m1@x>")),
            ],
        );
        assert_eq!(
            thread_of(&outcome, "imap:a:m1"),
            thread_of(&outcome, "imap:a:m2")
        );
        assert!(outcome.aliases.is_empty());
    }

    #[test]
    fn an_orphan_reply_then_its_parent_still_thread_together() {
        // The reply arrives first, referencing a parent not yet seen. The
        // parent arriving later must join the SAME thread.
        let mut state = ThreadState::new();
        let outcome = thread_batch(
            &mut state,
            &[
                input("imap:a:m2", "<m2@x>", Some("<m1@x>"), Some("<m1@x>")),
                input("imap:a:m1", "<m1@x>", None, None),
            ],
        );
        assert_eq!(
            thread_of(&outcome, "imap:a:m1"),
            thread_of(&outcome, "imap:a:m2")
        );
    }

    #[test]
    fn a_late_message_merges_two_threads_older_id_survives() {
        let mut state = ThreadState::new();
        let first = thread_batch(
            &mut state,
            &[
                input("imap:a:m1", "<m1@x>", None, None),
                input("imap:a:m2", "<m2@x>", None, None),
            ],
        );
        let t1 = thread_of(&first, "imap:a:m1").to_string();
        let t2 = thread_of(&first, "imap:a:m2").to_string();
        assert_ne!(t1, t2, "two unrelated messages are two threads");

        let merge = thread_batch(
            &mut state,
            &[input(
                "imap:a:m3",
                "<m3@x>",
                Some("<m2@x>"),
                Some("<m1@x> <m2@x>"),
            )],
        );
        // m3 joined the surviving (older) thread, which is t1.
        assert_eq!(thread_of(&merge, "imap:a:m3"), t1);
        assert_eq!(merge.aliases, vec![(t2.clone(), t1.clone())]);
        assert!(merge.changed_threads.contains(&t1));
        assert!(merge.changed_threads.contains(&t2));
    }

    #[test]
    fn two_same_subject_unrelated_messages_stay_separate() {
        // No subject merging: identical subjects with no shared references are
        // two threads.
        let mut state = ThreadState::new();
        let a = input("imap:a:m1", "<m1@x>", None, None);
        let b = input("imap:a:m2", "<m2@x>", None, None);
        let outcome = thread_batch(&mut state, &[a, b]);
        assert_ne!(
            thread_of(&outcome, "imap:a:m1"),
            thread_of(&outcome, "imap:a:m2")
        );
    }

    #[test]
    fn a_duplicate_message_id_resolves_to_one_thread() {
        let mut state = ThreadState::new();
        let outcome = thread_batch(
            &mut state,
            &[
                input("imap:a:dup", "<dup@x>", None, None),
                input("imap:a:dup", "<dup@x>", None, None),
            ],
        );
        let threads: std::collections::BTreeSet<_> =
            outcome.assignments.iter().map(|(_, t)| t.clone()).collect();
        assert_eq!(threads.len(), 1, "the same Message-ID is one thread");
    }

    #[test]
    fn a_missing_message_id_still_threads_via_its_stable_id() {
        let mut state = ThreadState::new();
        let a = ThreadingInput {
            message_id: "imap:a:h1".into(),
            message_id_header: None,
            in_reply_to: None,
            references: None,
        };
        let b = ThreadingInput {
            message_id: "imap:a:h2".into(),
            message_id_header: None,
            in_reply_to: None,
            references: None,
        };
        let outcome = thread_batch(&mut state, &[a, b]);
        assert_ne!(
            thread_of(&outcome, "imap:a:h1"),
            thread_of(&outcome, "imap:a:h2")
        );
    }

    #[test]
    fn a_hostile_references_header_is_capped_and_the_drop_is_reported() {
        let mut state = ThreadState::new();
        let parent = thread_batch(&mut state, &[input("imap:a:p", "<near@x>", None, None)]);
        let parent_thread = thread_of(&parent, "imap:a:p").to_string();

        let huge: String = (0..policy::MAX_REFERENCES + 50)
            .map(|i| {
                if i == policy::MAX_REFERENCES + 49 {
                    "<near@x>".to_string()
                } else {
                    format!("<old{i}@x>")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let outcome = thread_batch(
            &mut state,
            &[input("imap:a:child", "<child@x>", None, Some(&huge))],
        );
        assert!(outcome.references_dropped > 0, "over-cap drop is reported");
        // The nearest ancestor <near@x> is at the end, so it survived the cap
        // and the child threads with the parent.
        assert_eq!(thread_of(&outcome, "imap:a:child"), parent_thread);
    }

    #[test]
    fn threading_is_deterministic_across_two_identical_runs() {
        let messages = vec![
            input("imap:a:m1", "<m1@x>", None, None),
            input("imap:a:m2", "<m2@x>", Some("<m1@x>"), Some("<m1@x>")),
            input("imap:a:m3", "<m3@x>", None, None),
        ];
        let mut s1 = ThreadState::new();
        let o1 = thread_batch(&mut s1, &messages);
        let mut s2 = ThreadState::new();
        let o2 = thread_batch(&mut s2, &messages);
        assert_eq!(o1, o2);
    }

    #[test]
    fn seeded_prior_threads_are_the_ones_that_survive_a_merge() {
        let mut state = ThreadState::new();
        state.seed("imap:t:survivor", 0, &["old@x".to_string()]);
        let outcome = thread_batch(
            &mut state,
            &[
                input("imap:a:new", "<new@x>", None, None),
                input(
                    "imap:a:link",
                    "<link@x>",
                    Some("<new@x>"),
                    Some("<old@x> <new@x>"),
                ),
            ],
        );
        assert_eq!(thread_of(&outcome, "imap:a:link"), "imap:t:survivor");
        assert!(outcome
            .aliases
            .iter()
            .any(|(_, new_id)| new_id == "imap:t:survivor"));
    }
}
