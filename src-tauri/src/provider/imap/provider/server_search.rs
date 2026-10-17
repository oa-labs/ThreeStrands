//! Server search: running a rendered IMAP search plan across mailboxes and
//! mapping its outcome onto the engine's [`SearchReport`].

use crate::provider::{
    NoMatchSearchReason, PartialSearchReason, ProviderError, ProviderResult, SearchReport,
    SearchReportStatus, SkipSearchReason,
};

use crate::provider::imap::plan::SyncPlan;
use crate::provider::imap::policy::{self};
use crate::provider::imap::rfc822::to_raw_message;
use crate::provider::imap::search::{
    self, render_imap, ImapSearchContext, ImapSearchOutcome, LocalFilter,
};

use super::{db_err, AcquireError, ImapProvider, ImapSearchResult, PartialReason};

impl ImapProvider {
    /// The rich IMAP server-search outcome (Phase 5 run 4), the method
    /// [`MailSync::search`] wraps. See `docs/imap-design.md` ("Server search").
    ///
    /// Shape, matching the brief:
    /// 1. Parse + render ([`render_imap`]) against a context built from this
    ///    account's sync plan and settings. A `Skipped` (a term the server
    ///    cannot honestly evaluate — negation, invalid chars, over-limit, empty)
    ///    or `NoServerMatches` (an unknown label/folder/mailbox) returns an
    ///    EMPTY result WITHOUT opening a session — no network call at all. The
    ///    reason is logged at info; the query text never above debug.
    /// 2. For a `Plan`, open ONE session and reuse it for every mailbox's
    ///    EXAMINE + `UID SEARCH` + acquire. A planned mailbox that is not
    ///    `synced_now` is skipped and reported (a pin there would never be
    ///    maintained). Each mailbox is EXAMINEd (read-only — never SELECT, so
    ///    search never sets `\Seen`), its hits taken NEWEST-UID-FIRST.
    /// 3. Budget: at most [`policy::MAX_SEARCH_ACQUIRE`] hits across ALL
    ///    mailboxes. The budget is split fairly with
    ///    [`policy::search_acquire_share`] (`ceil(budget / mailboxes)` each,
    ///    newest-first within a mailbox) and the GLOBAL running total is the
    ///    hard stop, so the sum never exceeds the budget.
    /// 4. Deadline: a total wall-clock [`policy::SEARCH_DEADLINE`] (injectable
    ///    in tests) across all mailboxes' SEARCH + acquire. On expiry the driver
    ///    stops starting new mailboxes and returns what it acquired as a PARTIAL
    ///    result, never an error. Driven by `tokio::time` so a test can expire
    ///    it under paused time.
    /// 5. The chosen UIDs are acquired on the SAME session with
    ///    [`ImapProvider::acquire_uids`], passing the UIDVALIDITY read from THIS
    ///    search's EXAMINE. A UIDVALIDITY change between EXAMINE and acquire is
    ///    a clean per-mailbox SKIP (reported as a mailbox error, the others
    ///    still run) — never a wrong-incarnation ingest.
    /// 6. [`LocalFilter::HasAttachment`] is applied to the ACQUIRED threads
    ///    (decided from the stored MIME structure). Filtered-out threads stay
    ///    acquired and pinned (harmless — they expire) but are not returned.
    /// 7. The session is logged out AND dropped before returning, so the search
    ///    never holds a connection across anything that opens another (run 3's
    ///    live deadlock). `search` never calls sync.
    /// 8. Returns deduplicated thread ids in a stable NEWEST-FIRST order (by
    ///    acquired UID, mailboxes in plan order). These are the ids
    ///    `fetch_thread` resolves (aliases applied here as `acquire_uids` does).
    pub(super) async fn search_detailed(
        &self,
        query: &str,
        _page: Option<&str>,
    ) -> ProviderResult<ImapSearchResult> {
        // 1. Parse + render against this account's plan + settings.
        let plan = self.build_sync_plan()?;
        let delimiter = self.delimiter()?;
        let context = ImapSearchContext {
            plan: &plan,
            label_storage: self.settings.label_storage,
            label_container: self.settings.label_container.as_deref(),
            delimiter: Some(delimiter.as_str()),
        };
        let parsed = crate::search_query::SearchQuery::parse(query);
        match render_imap(&parsed, &context) {
            ImapSearchOutcome::Skipped { reasons } => {
                // A term the server cannot honestly evaluate: keep local
                // results only, make NO network call. Log the reason, not the
                // query text, above debug.
                log::info!(
                    "imap search: skipped server search ({} reason(s)); local results only",
                    reasons.len()
                );
                log::debug!("imap search: skipped query {query:?}: {reasons:?}");
                Ok(empty_search_result(Some(reasons), None))
            }
            ImapSearchOutcome::NoServerMatches { reason } => {
                log::info!("imap search: server proves no matches");
                log::debug!("imap search: no-match query {query:?}: {reason:?}");
                Ok(empty_search_result(None, Some(reason)))
            }
            ImapSearchOutcome::Plan(search_plan) => {
                self.run_search_plan(&plan, search_plan).await
            }
        }
    }

    /// Execute a rendered [`search::ImapSearchPlan`] against ONE session: the
    /// steps 2–8 above. Split out so `search_detailed` stays the parse/render
    /// decision and this owns the network + budget + deadline loop.
    pub(super) async fn run_search_plan(
        &self,
        plan: &SyncPlan,
        search_plan: search::ImapSearchPlan,
    ) -> ProviderResult<ImapSearchResult> {
        let deadline = tokio::time::Instant::now()
            + self.search_deadline_override.unwrap_or(policy::SEARCH_DEADLINE);
        let mailboxes_planned = search_plan.mailboxes.len();
        let per_mailbox = policy::search_acquire_share(policy::MAX_SEARCH_ACQUIRE, mailboxes_planned);
        let want_attachment = search_plan
            .local_filters
            .contains(&LocalFilter::HasAttachment);

        let mut session = match tokio::time::timeout_at(deadline, self.open_session()).await {
            Ok(result) => result?,
            Err(_) => {
                let mut result = empty_search_result(None, None);
                result.partial = Some(PartialReason::DeadlineExpired {
                    mailboxes_searched: 0,
                    mailboxes_planned,
                });
                return Ok(result);
            }
        };

        // Ordered (thread_id, deciding-uid) pairs, newest-UID-first within a
        // mailbox and mailboxes in plan order, deduplicated globally.
        let mut ordered_threads: Vec<String> = Vec::new();
        let mut seen_threads: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        let mut acquired_total = 0usize;
        let mut skipped_over_cap = 0usize;
        let mut unsynced_mailboxes: Vec<String> = Vec::new();
        let mut partial: Option<PartialReason> = None;
        let mut mailboxes_searched = 0usize;
        // Set when the deadline cancelled a command mid-flight. The connection
        // then holds a half-read response, so LOGOUT would be parsed against
        // stale data and, against a stalled server, wait forever while this
        // search holds the account's sync gate. Such a session is dropped
        // (closing the socket and releasing the lease) without LOGOUT.
        let mut connection_poisoned = false;

        for wire in search_plan.mailboxes.iter() {
            // Deadline: stop STARTING a new mailbox once the budget is spent;
            // return what we have as a PARTIAL result (not an error).
            if tokio::time::Instant::now() >= deadline {
                partial = Some(PartialReason::DeadlineExpired {
                    mailboxes_searched,
                    mailboxes_planned,
                });
                break;
            }
            // Global budget already spent: nothing more to acquire.
            if acquired_total >= policy::MAX_SEARCH_ACQUIRE {
                partial = Some(PartialReason::BudgetReached);
                break;
            }

            // Resolve the plan entry; a mailbox that is not synced_now is
            // skipped and reported (a pin there would never be maintained).
            let Some(entry) = plan.entries.iter().find(|e| &e.mailbox == wire) else {
                // The renderer only names mailboxes from the plan, so this is
                // unreachable in practice; treat an absent entry as unsynced.
                unsynced_mailboxes.push(wire.clone());
                continue;
            };
            if !entry.synced_now {
                unsynced_mailboxes.push(wire.clone());
                continue;
            }

            // 2. EXAMINE (read-only) + UID SEARCH, under the deadline.
            let examined = tokio::time::timeout_at(deadline, async {
                let status = session.examine(wire).await?;
                let uidvalidity = status.uid_validity.unwrap_or(0) as i64;
                let hits = session.uid_search(&search_plan.criteria).await?;
                Ok::<(i64, Vec<u32>), ProviderError>((uidvalidity, hits))
            })
            .await;
            let (uidvalidity, mut hits) = match examined {
                Ok(Ok(pair)) => pair,
                // Deadline hit mid-EXAMINE/SEARCH: partial, stop.
                Err(_elapsed) => {
                    connection_poisoned = true;
                    partial = Some(PartialReason::DeadlineExpired {
                        mailboxes_searched,
                        mailboxes_planned,
                    });
                    break;
                }
                // A server error on THIS mailbox. Decision: PARTIAL, not error
                // — any earlier mailbox's hits are already acquired in their
                // own committed transactions, so discarding them by failing the
                // whole search would waste committed work and show the user
                // nothing. Stop here and report which mailbox failed.
                Ok(Err(error)) => {
                    log::warn!("imap search: mailbox {wire:?} failed, returning partial: {error}");
                    partial = Some(PartialReason::MailboxError {
                        mailbox: wire.clone(),
                        error: error.to_string(),
                    });
                    break;
                }
            };
            mailboxes_searched += 1;

            // Take hits NEWEST-UID-FIRST, up to this mailbox's fair share AND
            // the global remaining budget (the hard stop).
            hits.sort_unstable();
            hits.reverse();
            let remaining = policy::MAX_SEARCH_ACQUIRE - acquired_total;
            let take = per_mailbox.min(remaining);
            if hits.len() > take {
                // More hits than we will acquire this mailbox: budget-limited.
                partial.get_or_insert(PartialReason::BudgetReached);
            }
            let chosen: Vec<u32> = hits.into_iter().take(take).collect();
            if chosen.is_empty() {
                continue;
            }

            // 5. Acquire the chosen UIDs on the SAME session, passing THIS
            // EXAMINE's UIDVALIDITY. A UIDVALIDITY change between EXAMINE and
            // acquire is a clean per-mailbox skip — the other mailboxes still
            // run — never a wrong-incarnation ingest.
            let acquire = tokio::time::timeout_at(
                deadline,
                self.acquire_uids(session.as_mut(), entry, &chosen, uidvalidity),
            )
            .await;
            let outcome = match acquire {
                Ok(Ok(outcome)) => outcome,
                Err(_elapsed) => {
                    connection_poisoned = true;
                    partial = Some(PartialReason::DeadlineExpired {
                        mailboxes_searched,
                        mailboxes_planned,
                    });
                    break;
                }
                Ok(Err(AcquireError::UidValidityMismatch { expected, found, .. })) => {
                    log::warn!(
                        "imap search: {wire:?} UIDVALIDITY changed ({expected} -> {found}) between \
                         EXAMINE and acquire; skipping this mailbox cleanly"
                    );
                    partial.get_or_insert(PartialReason::MailboxError {
                        mailbox: wire.clone(),
                        error: format!("UIDVALIDITY changed ({expected} -> {found})"),
                    });
                    continue;
                }
                Ok(Err(AcquireError::MailboxNotSynced { mailbox })) => {
                    unsynced_mailboxes.push(mailbox);
                    continue;
                }
                Ok(Err(AcquireError::Provider(error))) => return Err(error),
            };
            skipped_over_cap += outcome.skipped_over_cap;
            acquired_total += chosen.len();

            // Resolve each chosen UID -> thread id in NEWEST-FIRST UID order,
            // appending to the global ordered/dedup list. `acquire_uids`
            // returns threads sorted alphabetically, which is NOT the newest-
            // first order we want, so resolve per-UID here (the location rows
            // were just committed by the acquire).
            for &uid in &chosen {
                let Some(message_id) = self
                    .store
                    .location_message_id(wire, uidvalidity, uid as i64)
                    .map_err(db_err)?
                else {
                    continue; // expunged between SEARCH and FETCH: no location.
                };
                let Some(thread_id) = self.store.thread_of_message(&message_id).map_err(db_err)?
                else {
                    continue;
                };
                let resolved = self.store.resolve_thread_alias(&thread_id).map_err(db_err)?;
                if seen_threads.insert(resolved.clone()) {
                    ordered_threads.push(resolved);
                }
            }
        }

        // 7. Logout AND drop the session before anything else (the body reads
        // for the attachment filter and the caller's later sync must not race a
        // held connection lease — run 3's deadlock).
        if !connection_poisoned {
            // Cleanup shares the search budget. A stalled LOGOUT must not
            // keep the account's sync gate held after results are ready.
            let _ = tokio::time::timeout_at(deadline, session.logout()).await;
        }
        drop(session);

        // 6. Apply has:attachment to the acquired threads, from the stored MIME
        // structure. Filtered-out threads stay acquired/pinned (they expire).
        let thread_ids = if want_attachment {
            let mut kept = Vec::new();
            for thread_id in ordered_threads {
                if self.thread_has_attachment(&thread_id)? {
                    kept.push(thread_id);
                }
            }
            kept
        } else {
            ordered_threads
        };

        Ok(ImapSearchResult {
            thread_ids,
            partial,
            skipped: None,
            no_matches: None,
            unsynced_mailboxes,
            skipped_over_cap,
        })
    }

    /// Whether any message in a thread has an attachment, decided from the
    /// STORED MIME structure (the cached raw bytes the acquire just wrote), the
    /// provider-neutral way: parse each cached body to a [`RawMessage`] and ask
    /// the shared [`crate::mime::normalize`] whether it produced any
    /// attachments. A message with no cached body (expunged between SEARCH and
    /// read) contributes nothing; a body that fails to parse is treated as "no
    /// attachment" (we only KEEP a thread we can PROVE has one). No network:
    /// this reads the local cache only, after the session is dropped.
    fn thread_has_attachment(&self, thread_id: &str) -> ProviderResult<bool> {
        let resolved = self.store.resolve_thread_alias(thread_id).map_err(db_err)?;
        for message_id in self.store.messages_in_thread(&resolved).map_err(db_err)? {
            let Some(cached) = self.cache.get(&message_id).map_err(db_err)? else {
                continue;
            };
            // Reuse the ingest parser; a per-message oversize/parse failure is
            // not proof of an attachment, so skip it rather than fail.
            let raw = match to_raw_message(&cached.raw, &message_id, Vec::new()) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            match crate::mime::normalize(&raw) {
                Ok(normalized) if !normalized.attachments.is_empty() => return Ok(true),
                _ => {}
            }
        }
        Ok(false)
    }
}

/// An empty [`ImapSearchResult`] for a `Skipped`/`NoServerMatches` render: no
/// threads, no partial, carrying the notice(s) for run 5 to word.
fn empty_search_result(
    skipped: Option<Vec<search::SearchNotice>>,
    no_matches: Option<search::SearchNotice>,
) -> ImapSearchResult {
    ImapSearchResult {
        thread_ids: Vec::new(),
        partial: None,
        skipped,
        no_matches,
        unsynced_mailboxes: Vec::new(),
        skipped_over_cap: 0,
    }
}

/// Map one IMAP [`ImapSearchResult`] to the provider-neutral [`SearchReport`]
/// the UI words (Phase 5 run 5). Precedence mirrors `search_detailed`'s own:
///
/// 1. a `Skipped` render (a term the server cannot honestly evaluate) →
///    [`SearchReportStatus::Skipped`];
/// 2. else a `NoServerMatches` render → [`SearchReportStatus::NoServerMatches`];
/// 3. else a partial run (deadline / budget / a folder erroring) →
///    [`SearchReportStatus::Partial`];
/// 4. else [`SearchReportStatus::Complete`].
///
/// The internal [`search::SearchNotice`] / [`PartialReason`] detail is
/// collapsed to the small typed codes the UI maps to copy — the user's query,
/// mailbox wire names and developer strings never cross this boundary. The
/// unsynced-mailbox and over-cap counts ride on the report regardless of
/// status, so a `Complete` run that nonetheless skipped an unsynced folder is
/// still worded.
pub(super) fn imap_search_report(result: &ImapSearchResult) -> SearchReport {
    let unsynced_mailboxes = result.unsynced_mailboxes.len();
    let skipped_over_cap = result.skipped_over_cap;

    let status = if let Some(reasons) = &result.skipped {
        // Every skip notice collapses to the one user-facing "unsupported"
        // code: the UI never shows the internal reason or the query text.
        SearchReportStatus::Skipped {
            reasons: reasons
                .iter()
                .map(|_| SkipSearchReason::Unsupported)
                .collect(),
        }
    } else if result.no_matches.is_some() {
        SearchReportStatus::NoServerMatches {
            reason: NoMatchSearchReason::UnknownName,
        }
    } else if let Some(partial) = &result.partial {
        let mut reasons = vec![partial_reason_code(partial)];
        // Hits refused by the pin cap are matches the user will not see:
        // say so, worded like the budget limit (the newest were kept).
        if skipped_over_cap > 0 && !reasons.contains(&PartialSearchReason::Budget) {
            reasons.push(PartialSearchReason::Budget);
        }
        SearchReportStatus::Partial { reasons }
    } else if skipped_over_cap > 0 {
        // Every mailbox finished, but the pin cap refused some hits, so the
        // result is NOT complete. Never report a silent drop as Complete.
        SearchReportStatus::Partial {
            reasons: vec![PartialSearchReason::Budget],
        }
    } else {
        SearchReportStatus::Complete
    };

    let server_searched_accounts =
        usize::from(!matches!(status, SearchReportStatus::Skipped { .. }));
    SearchReport {
        status,
        unsynced_mailboxes,
        skipped_over_cap,
        server_searched_accounts,
    }
}

/// Collapse a [`PartialReason`] to its UI code. The named mailbox and the
/// error string on `MailboxError` are deliberately dropped — a wire name or a
/// server error string must never reach the UI.
fn partial_reason_code(partial: &PartialReason) -> PartialSearchReason {
    match partial {
        PartialReason::DeadlineExpired { .. } => PartialSearchReason::Deadline,
        PartialReason::BudgetReached => PartialSearchReason::Budget,
        PartialReason::MailboxError { .. } => PartialSearchReason::FolderError,
    }
}
