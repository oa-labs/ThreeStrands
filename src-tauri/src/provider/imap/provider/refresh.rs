//! The sync seam: refreshing every synced mailbox against the server, ingesting
//! new mail, loading thread state, and the [`MailSync`] impl the engine polls.

use async_trait::async_trait;

use crate::mime::RawMessage;
use crate::provider::{
    MailSync, ProviderError, ProviderResult, SearchReport, SyncBatch, SyncCursor, ThreadPage,
};

use crate::provider::imap::fetch::{fetch_identity, SessionBodyFetcher};
use crate::provider::imap::labels::{labels_for_location_ids, merge_label_sets};
use crate::provider::imap::plan::{self, CatalogMailbox, SyncPlan};
use crate::provider::imap::policy::{self, SyncLimits};
use crate::provider::imap::rfc822::to_raw_message;
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::threading::{self, ThreadState, ThreadingInput};
use crate::provider::imap::{ImapLocation, SyncRoundWrite};

use super::server_search::imap_search_report;
use super::{
    chunk, db_err, is_transient, uid_set, AcquireError, AcquireOutcome, ImapProvider,
    IngestOutcome, Refreshed, INBOX,
};

impl ImapProvider {
    /// Refresh every synced mailbox against the server and return the new
    /// generation. INBOX is swept first and every poll; the other synced
    /// mailboxes (Trash, Junk, then Sent) follow in plan order with cheap
    /// cadence gating. Each mailbox round is its OWN atomic commit, so a
    /// failure in one mailbox leaves the others' committed work intact and the
    /// at-least-once cursor contract whole.
    pub(super) async fn refresh_all(&self) -> ProviderResult<Refreshed> {
        let mut session = self.open_session().await?;
        let result = self.refresh_all_with(session.as_mut()).await;
        let _ = session.logout().await;
        result
    }

    /// The full plan walk against an already-open session. INBOX first (always
    /// a full sweep), then each other `synced_now` mailbox in plan order up to
    /// the per-poll budget, each with cadence gating. The last-committed
    /// generation is returned. INBOX failures propagate (nothing is ingested
    /// without INBOX); a failing folder is logged and skipped, since mailboxes
    /// that already committed are durable and the folder is retried next poll.
    pub(super) async fn refresh_all_with(
        &self,
        session: &mut dyn ImapSession,
    ) -> ProviderResult<Refreshed> {
        // Catalog refresh (item 3): at baseline and whenever the account's
        // periodic sweep is due, LIST the server and upsert NAME/DELIMITER/
        // SPECIAL_USE so a mailbox created after setup (a new Sent/Trash/Junk)
        // is noticed and the plan picks it up. Catalog-only — never clobbers
        // counters/write-capabilities. A failure here is logged and IGNORED:
        // it must never abort a poll (INBOX mail keeps flowing).
        self.refresh_catalog_if_due(session).await;

        let plan = self.build_sync_plan()?;
        // INBOX first and always a full sweep.
        let inbox_entry = plan
            .entries
            .iter()
            .find(|e| e.is_inbox)
            .cloned()
            // If the catalog has no INBOX row yet (first ever sync before
            // discovery persisted it), synthesize an INBOX plan entry.
            .unwrap_or(plan::PlanEntry {
                mailbox: INBOX.to_string(),
                role: None,
                is_inbox: true,
                window_class: policy::SyncWindowClass::Inbox,
                label_kind: plan::LabelKind::SystemRole,
                location_label_id: Some("INBOX".to_string()),
                synced_now: true,
            });
        let mut generation = self.refresh_mailbox_with(session, &inbox_entry, true).await?;

        // FAIR SCHEDULING (Slice 5b-2). The other synced mailboxes are chosen
        // per poll by `policy::schedule_visits`, NOT by a fixed prefix: every
        // synced SYSTEM-ROLE mailbox (Sent, Trash, Junk, Archive) is visited
        // every poll (bounded, cheap when idle via the cadence gate), then
        // user/label folders LEAST-RECENTLY-ATTEMPTED first up to the remaining
        // FOLDER_ROUNDS_PER_POLL budget. Over MAX_SYNCED_MAILBOXES the set is
        // capped (system roles first, then by name) and the drop is LOGGED.
        let synced_non_inbox: Vec<plan::PlanEntry> =
            plan.synced().filter(|e| !e.is_inbox).cloned().collect();

        // Candidates for the cap + scheduler, carrying each mailbox's persisted
        // attempt order, successful visit clock and coverage stamp. Attempts
        // rotate even on failure; only successful visits count toward coverage.
        let current_epoch = self.store.complete_walks().map_err(db_err)? as i64;
        let mut candidates: Vec<policy::ScheduleCandidate> = Vec::new();
        for entry in &synced_non_inbox {
            candidates.push(policy::ScheduleCandidate {
                mailbox: entry.mailbox.clone(),
                last_attempt: self
                    .store
                    .mailbox_last_attempt(&entry.mailbox)
                    .map_err(db_err)?,
                last_visited_at: self.store.mailbox_last_visited_at(&entry.mailbox).map_err(db_err)?,
                visited_in_epoch: self.store.mailbox_visited_in_epoch(&entry.mailbox).map_err(db_err)?,
                is_system_role: entry.role.is_some(),
            });
        }
        // TEST KNOB: inject synthetic synced candidates so a fairness test can
        // exercise the scheduler with many user folders without a huge plan.
        // Never set in production.
        if let Some(extra) = &self.extra_synced_candidates {
            candidates.extend(extra.iter().cloned());
        }

        // Cap the synced set; LOG the count dropped, never silently.
        let (allowed, dropped) =
            policy::cap_synced_mailboxes(&candidates, policy::MAX_SYNCED_MAILBOXES);
        if dropped > 0 {
            log::warn!(
                "imap sync: {dropped} synced mailbox(es) over the {}-mailbox cap are not synced this poll",
                policy::MAX_SYNCED_MAILBOXES
            );
        }
        let allowed_set: std::collections::BTreeSet<&str> =
            allowed.iter().map(String::as_str).collect();
        let allowed_candidates: Vec<policy::ScheduleCandidate> = candidates
            .iter()
            .filter(|c| allowed_set.contains(c.mailbox.as_str()))
            .cloned()
            .collect();

        // The per-poll visit order (system roles first, then LRV folders up to
        // the budget). A COVERAGE EPOCH is coverage ACROSS polls, not a single
        // poll that visited every mailbox: run B syncs more user/label folders
        // than one poll's budget can visit, so no single poll covers them all.
        // Each mailbox visited this poll stamped `visited_in_epoch` with the
        // current epoch (inside refresh_mailbox_with). After the poll, the
        // epoch advances iff INBOX plus every allowed real plan entry has
        // been successfully visited in the current epoch. Capped-out entries
        // cannot hold coverage open because they are never scheduled.
        // Synthetic test candidates are NOT server-backed and are excluded.
        let budget = self
            .folder_rounds_per_poll_override
            .unwrap_or(policy::FOLDER_ROUNDS_PER_POLL);
        let visit_order = policy::schedule_visits(&allowed_candidates, current_epoch, budget);
        let entry_by_name: std::collections::BTreeMap<&str, &plan::PlanEntry> =
            synced_non_inbox.iter().map(|e| (e.mailbox.as_str(), e)).collect();
        let scheduled_real: Vec<&plan::PlanEntry> = visit_order
            .iter()
            .filter_map(|name| entry_by_name.get(name.as_str()).copied())
            .collect();
        for entry in scheduled_real {
            self.store
                .record_mailbox_attempt(&entry.mailbox)
                .map_err(db_err)?;
            // Cadence gating decides whether a full sweep is needed. Only INBOX
            // may abort a poll: the engine ingests a poll's changes only after
            // the whole poll succeeds, so a folder that cannot be synced (its
            // mailbox renamed or deleted on the server, say) must not stop mail
            // flowing. A folder failure is logged and retried next poll, and a
            // dropped connection ends the folder walk (INBOX already committed).
            // Only a credential rejection, which pauses the account, propagates.
            // A folder that errors is simply not stamped for this epoch, so the
            // epoch stays open until it succeeds (the safe direction: the
            // emptied-thread grace never advances early).
            match self.refresh_mailbox_with(session, entry, false).await {
                Ok(committed) => generation = committed,
                Err(error) if error.requires_reauthentication() => return Err(error),
                Err(error) => {
                    log::warn!(
                        "imap sync: skipping mailbox {:?} this poll: {error}",
                        entry.mailbox
                    );
                    if is_transient(&error) {
                        break;
                    }
                }
            }
        }

        // End-of-poll coverage check (Slice 5b-2 item 4). The full currently-
        // synced set is INBOX plus every real entry allowed by the cap (NOT
        // just this poll's schedule, and NOT synthetic test candidates). The epoch
        // advances only when every one has been visited in the current epoch;
        // on advance, emptied hot threads whose grace just expired are journaled
        // (same transaction) — bumping the generation only when something
        // actually expired. Visit recording never bumps the generation.
        let mut synced_all: Vec<String> = Vec::with_capacity(synced_non_inbox.len() + 1);
        synced_all.push(inbox_entry.mailbox.clone());
        synced_all.extend(
            synced_non_inbox
                .iter()
                .filter(|e| allowed_set.contains(e.mailbox.as_str()))
                .map(|e| e.mailbox.clone()),
        );
        let (after_epoch, outstanding) = self
            .store
            .advance_coverage_epoch_if_complete(&synced_all)
            .map_err(db_err)?;
        generation = generation.max(after_epoch);
        if !outstanding.is_empty() {
            // Coverage still open this epoch: the emptied-thread grace cannot
            // advance until these mailboxes are successfully visited. This
            // fires at most once per poll and only while coverage is genuinely
            // incomplete. A mailbox that VANISHED from the server no longer
            // holds the epoch open forever — the catalog refresh retires it
            // (item 5 / `retire_vanished_mailboxes`), so it drops out of the
            // synced set. A mailbox that is still LISTed but transiently fails
            // EXAMINE legitimately holds the epoch open until it recovers (the
            // safe direction: the grace never advances early).
            log::warn!(
                "imap sync: coverage epoch still open — {} mailbox(es) not yet visited this epoch: {:?}",
                outstanding.len(),
                outstanding
            );
        }
        Ok(Refreshed { last: generation })
    }

    /// Build the account's sync plan from the persisted catalog + settings.
    pub(super) fn build_sync_plan(&self) -> ProviderResult<SyncPlan> {
        let rows = self.store.mailboxes().map_err(db_err)?;
        let delimiter = rows
            .iter()
            .find_map(|row| row.delimiter.clone())
            .unwrap_or_else(|| "/".to_string());
        let catalog: Vec<CatalogMailbox> = rows
            .iter()
            .map(|row| CatalogMailbox {
                name: row.name.clone(),
                special_use: row.special_use.clone(),
            })
            .collect();
        let mut plan = plan::build_plan(&catalog, Some(&delimiter), &self.settings);
        // TEST KNOB: re-class named mailboxes as synced USER FOLDERS (role
        // None, synced_now true) so the coverage-epoch logic can be exercised
        // with real server-backed folders that are subject to the per-poll
        // budget (run B produces such entries naturally).
        if let Some(forced) = &self.force_user_folders {
            for entry in plan.entries.iter_mut() {
                if forced.contains(&entry.mailbox) {
                    entry.role = None;
                    entry.synced_now = true;
                    entry.label_kind = plan::LabelKind::UserFolder;
                    entry.window_class = policy::SyncWindowClass::Folder;
                    entry.location_label_id = Some(format!("folder:{}", entry.mailbox));
                }
            }
        }
        Ok(plan)
    }

    /// Round-time catalog refresh (item 3) + vanished-mailbox retirement
    /// (item 5). When the account's periodic catalog sweep is due (always true
    /// at baseline), `LIST` the server and record NAME/DELIMITER/SPECIAL_USE
    /// for every SELECTABLE mailbox via the catalog-only upsert, so a mailbox
    /// created after account setup is noticed and the plan can act on it.
    /// `\Noselect` containers are skipped (they are never locations). After a
    /// SUCCESSFUL non-empty `LIST`, any catalog row the server no longer lists
    /// (and that is not INBOX) is RETIRED — its catalog row, sync-state row and
    /// locations are deleted and its hot threads journaled — so a deleted
    /// Proton label/folder stops failing EXAMINE forever and holding the
    /// coverage epoch open. An empty `LIST` is treated as a failure and retires
    /// nothing. Any failure — the `LIST` itself, a single upsert, or the
    /// retirement — is logged and swallowed, because this must NEVER abort a
    /// poll: if it did, a transient `LIST` error would stop INBOX mail flowing.
    async fn refresh_catalog_if_due(&self, session: &mut dyn ImapSession) {
        let now = (self.now)();
        match self.store.catalog_refresh_due(now) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                log::warn!("imap sync: catalog refresh skipped (state read failed): {error}");
                return;
            }
        }
        let entries = match session.list_mailboxes().await {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!("imap sync: catalog refresh LIST failed, ignored this poll: {error}");
                return;
            }
        };
        // The SELECTABLE mailboxes the server reported this LIST (the catalog
        // only ever holds selectable rows; `\Noselect` containers are never
        // catalog locations). This is both the upsert set and the "present"
        // set for vanished-mailbox retirement.
        let present: Vec<String> = entries
            .iter()
            .filter(|entry| !entry.no_select)
            .map(|entry| entry.name.clone())
            .collect();
        for entry in &entries {
            if entry.no_select {
                continue; // \Noselect containers are never catalog locations.
            }
            if let Err(error) = self.store.upsert_mailbox_catalog(
                &entry.name,
                entry.delimiter.as_deref(),
                entry.special_use.as_deref(),
            ) {
                log::warn!(
                    "imap sync: catalog refresh could not record {:?}, ignored: {error}",
                    entry.name
                );
            }
        }
        // Vanished-mailbox retirement (item 5 / fact 5). A SUCCESSFUL LIST that
        // returned at least one selectable mailbox lets us retire any catalog
        // row the server no longer lists: it would otherwise fail EXAMINE
        // forever and hold the coverage epoch — and the emptied-thread grace —
        // open indefinitely. An empty `present` is treated as a FAILURE (a
        // server that momentarily lists nothing must not wipe the catalog), so
        // we skip retirement entirely; `retire_vanished_mailboxes` guards this
        // too. INBOX is never retired.
        if present.is_empty() {
            log::warn!(
                "imap sync: catalog refresh LIST returned no selectable mailboxes; \
                 treating as a failure and retiring nothing this poll"
            );
        } else {
            match self.store.retire_vanished_mailboxes(&present) {
                Ok(retired) if !retired.is_empty() => {
                    log::info!(
                        "imap sync: retired {} vanished mailbox(es) from the catalog: {:?}",
                        retired.len(),
                        retired
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    log::warn!(
                        "imap sync: vanished-mailbox retirement failed, ignored this poll: {error}"
                    );
                }
            }
        }
        if let Err(error) = self.store.record_catalog_refresh(now) {
            log::warn!("imap sync: catalog refresh clock not recorded: {error}");
        }
    }

    /// Run the INBOX round against an already-open session (kept for the
    /// engine-level tests and callers that drive INBOX directly). Equivalent to
    /// a forced full sweep of the INBOX plan entry.
    pub(super) async fn refresh_inbox_with(
        &self,
        session: &mut dyn ImapSession,
    ) -> ProviderResult<u64> {
        let entry = plan::PlanEntry {
            mailbox: INBOX.to_string(),
            role: None,
            is_inbox: true,
            window_class: policy::SyncWindowClass::Inbox,
            label_kind: plan::LabelKind::SystemRole,
            location_label_id: Some("INBOX".to_string()),
            synced_now: true,
        };
        self.refresh_mailbox_with(session, &entry, true).await
    }

    /// The sync round for ONE mailbox against an already-open session, driven
    /// by its plan entry. INBOX passes `force_sweep = true` (swept every poll);
    /// a non-INBOX mailbox passes `false` and the cheap cadence check decides
    /// whether to do the full SEARCH/FETCH or skip to just recording its
    /// counters.
    ///
    /// Every durable write this round decides — new/updated locations,
    /// deletions, thread assignments, aliases, newly-hot threads, the journal,
    /// the mailbox cadence counters and the generation bump — is collected into
    /// one [`SyncRoundWrite`] and applied in a SINGLE transaction at the end
    /// (`commit_sync_round`). A failure before that commit changes no mail,
    /// cadence counters or coverage for this mailbox. Attempt ordering is
    /// recorded separately before network work so retries rotate fairly.
    /// Body-cache puts happen eagerly
    /// during fetch because the cache is rebuildable and deduplicated by stable
    /// id. EXAMINE only, so sync never sets `\Seen`.
    pub(super) async fn refresh_mailbox_with(
        &self,
        session: &mut dyn ImapSession,
        entry: &plan::PlanEntry,
        force_sweep: bool,
    ) -> ProviderResult<u64> {
        let mailbox = entry.mailbox.as_str();
        // INBOX honors the provider's `limits` knob (tests shrink the window
        // there); every other mailbox uses its plan class's limits, with an
        // optional test override of the window ceiling (eviction semantics
        // preserved).
        let limits = if entry.is_inbox {
            self.limits
        } else {
            let mut limits = SyncLimits::for_class(entry.window_class);
            if let Some(window) = self.folder_window_override {
                limits.mailbox_window = window;
            }
            limits
        };
        // A thread is hot if it has a location in INBOX or Sent.
        let makes_hot = entry.is_inbox || matches!(entry.role, Some(crate::provider::imap::MailboxRole::Sent));
        // Sent is acquired in CHUNKS (item 2): newest-first, at most
        // SENT_ROUND_MESSAGES still-unacquired UIDs per round, with a NULL/UID
        // `backfill_low_uid` watermark marking completeness. It is non-evicting
        // (SyncLimits::sent) and its backfill-in-progress state must defeat the
        // cheap cadence skip below.
        let is_sent = matches!(entry.role, Some(crate::provider::imap::MailboxRole::Sent));

        // 1. EXAMINE (read-only) — never SELECT, so sync never sets \Seen.
        let status = session.examine(mailbox).await?;
        let server_uidvalidity = status.uid_validity.unwrap_or(0) as i64;
        let server_exists = status.exists as i64;
        let server_uidnext = status.uid_next.unwrap_or(0) as i64;

        // Cadence gating for a non-INBOX mailbox: if the EXAMINE counters are
        // unchanged AND the periodic sweep is not yet due, skip the UID
        // SEARCH/FETCH entirely. INBOX always sweeps (force_sweep). A
        // UIDVALIDITY change is NEVER skipped — it invalidates every local UID.
        // A Sent mailbox whose backfill is still INCOMPLETE is NEVER skipped
        // either (item 2): EXISTS/UIDNEXT unchanged does not mean the window is
        // fully acquired, so there is still a chunk to pull this poll.
        let now = (self.now)();
        // The COVERAGE EPOCH this successful visit belongs to (Slice 5b-2):
        // stamped into `visited_in_epoch` so coverage can be judged ACROSS
        // polls. Stable during a poll — only the end-of-poll
        // `advance_coverage_epoch_if_complete` changes it.
        let epoch = self.store.complete_walks().map_err(db_err)? as i64;
        let local_uidvalidity_pre = self
            .store
            .locations_in_mailbox(mailbox)
            .map_err(db_err)?
            .first()
            .map(|l| l.uidvalidity);
        let uidvalidity_changed =
            matches!(local_uidvalidity_pre, Some(stored) if stored != server_uidvalidity);
        let stored = self.store.mailbox_sync_state(mailbox).map_err(db_err)?;
        let backfill_incomplete = is_sent
            && policy::sent_backfill_incomplete(
                self.store.sent_backfill_low_uid(mailbox).map_err(db_err)?,
            );
        // A mailbox with a TTL-EXPIRED search pin must NOT be cheap-skipped:
        // the pin's release (and the possible aging-out of its now-unprotected
        // location) is work this poll owes even when EXISTS/UIDNEXT are
        // unchanged. Mirrors the Sent-backfill-incomplete defeat of the gate.
        let has_expired_pins = !uidvalidity_changed
            && !self
                .store
                .expired_pins(mailbox, server_uidvalidity, now - policy::SEARCH_PIN_TTL)
                .map_err(db_err)?
                .is_empty();
        let sweep_due = match stored {
            Some((_, _, last_sweep_at)) => policy::folder_sweep_due(last_sweep_at, now),
            None => true, // never synced -> always sweep
        };
        let counters_unchanged = match stored {
            Some((last_exists, last_uidnext, _)) => policy::folder_counters_unchanged(
                last_exists,
                last_uidnext,
                server_exists,
                server_uidnext,
            ),
            None => false,
        };
        if !force_sweep
            && !uidvalidity_changed
            && !backfill_incomplete
            && !has_expired_pins
            && counters_unchanged
            && !sweep_due
        {
            // Nothing to do: record the (unchanged) counters WITHOUT bumping
            // the generation, keeping last_sweep_at as stored.
            let last_sweep_at = stored.map(|(_, _, s)| s).unwrap_or(now);
            let mut round = SyncRoundWrite::default();
            // A cadence-gate no-op still COUNTS as a visit: stamp
            // last_visited_at=now AND visited_in_epoch=epoch so the fair
            // scheduler rotates this mailbox forward and the coverage epoch
            // counts it as visited, even though no full sweep ran (item 2/4).
            round.mailbox_state =
                Some((mailbox.to_string(), server_exists, server_uidnext, last_sweep_at, now, epoch));
            return self.store.commit_sync_round(&round).map_err(db_err);
        }
        // A full sweep is happening now: advance the sweep clock when due (or
        // when forced for a non-INBOX mailbox); INBOX does not track a sweep
        // clock meaningfully but recording `now` is harmless.
        let new_last_sweep_at = if sweep_due || force_sweep {
            now
        } else {
            stored.map(|(_, _, s)| s).unwrap_or(now)
        };

        // 2. Local view: stored UIDVALIDITY + per-UID flags for this mailbox.
        let local_locations = self.store.locations_in_mailbox(mailbox).map_err(db_err)?;
        let local_uidvalidity = local_locations.first().map(|l| l.uidvalidity);

        // 3. UID SEARCH ALL — the server's full UID set for this mailbox.
        let server_uids = session.uid_search("ALL").await?;

        let local_view = self.local_view(local_uidvalidity, &local_locations);
        let mut flag_uids: std::collections::BTreeSet<u32> =
            policy::select_window(server_uids.clone(), limits.mailbox_window)
                .in_window
                .into_iter()
                .collect();
        // Non-evicting mailboxes keep acquired locations beyond the acquisition
        // window. Continue refreshing their flags while they remain on the
        // server, but never carry UIDs across a UIDVALIDITY reset.
        if !limits.evicts && !uidvalidity_changed {
            let live_uids: std::collections::BTreeSet<u32> = server_uids.iter().copied().collect();
            flag_uids.extend(
                local_view
                    .flags_by_uid
                    .keys()
                    .filter(|uid| live_uids.contains(uid))
                    .copied(),
            );
        }

        // SEARCH PINS (schema v70, Phase 5 run 3). A pinned location is a
        // search-acquired message that is usually OLD — outside this evicting
        // mailbox's window — and must NOT age out. The pinned set is passed to
        // the delta so a pinned out-of-window UID is retained (deleted only
        // when the SERVER expunged it), and pinned UIDs still on the server get
        // their flags refreshed like a non-evicting mailbox's retained UIDs.
        //
        // On a UIDVALIDITY reset the old incarnation's pins are meaningless
        // (its UIDs are renumbered); they are dropped alongside the locations,
        // and the EFFECTIVE pin set for this round's delta is empty.
        //
        // TTL (SEARCH_PIN_TTL): a pin older than the TTL is RELEASED this round
        // and, once released, no longer protects its UID — it drops from the
        // effective set, so an out-of-window evicting location ages out
        // normally. Re-acquiring (the acquire path) refreshes `pinned_at`.
        let mut pin_releases: Vec<(String, i64, i64)> = Vec::new();
        let effective_pins: std::collections::BTreeSet<u32> = if uidvalidity_changed {
            // Reset: drop every pin under the OLD incarnation. The local UID's
            // uidvalidity (what we stored) is the incarnation whose pins die.
            if let Some(old_uidvalidity) = local_uidvalidity {
                for uid in self
                    .store
                    .pinned_uids(mailbox, old_uidvalidity)
                    .map_err(db_err)?
                {
                    pin_releases.push((mailbox.to_string(), old_uidvalidity, uid as i64));
                }
            }
            std::collections::BTreeSet::new()
        } else {
            let now_secs = now;
            let cutoff = now_secs - policy::SEARCH_PIN_TTL;
            let expired: std::collections::BTreeSet<u32> = self
                .store
                .expired_pins(mailbox, server_uidvalidity, cutoff)
                .map_err(db_err)?
                .into_iter()
                .collect();
            for uid in &expired {
                pin_releases.push((mailbox.to_string(), server_uidvalidity, *uid as i64));
            }
            let all_pins: std::collections::BTreeSet<u32> = self
                .store
                .pinned_uids(mailbox, server_uidvalidity)
                .map_err(db_err)?
                .into_iter()
                .collect();
            // Effective = pinned AND not expired. Refresh flags for every live
            // effective pin so a flag-only change on a pinned message is seen.
            let live_server: std::collections::BTreeSet<u32> =
                server_uids.iter().copied().collect();
            let effective: std::collections::BTreeSet<u32> =
                all_pins.difference(&expired).copied().collect();
            flag_uids.extend(effective.iter().filter(|uid| live_server.contains(uid)).copied());
            effective
        };

        let server_flags = self
            .fetch_flags(session, &flag_uids.into_iter().collect::<Vec<_>>())
            .await?;
        let server_view = crate::provider::imap::delta::ServerMailboxView {
            uidvalidity: server_uidvalidity,
            all_uids: server_uids.clone(),
            flags_by_uid: server_flags,
        };
        let mut delta =
            crate::provider::imap::delta::compute_delta(&local_view, &server_view, &limits, &effective_pins);

        if delta.beyond_window > 0 {
            log::info!(
                "imap sync: {} {mailbox} messages beyond the sync window",
                delta.beyond_window
            );
        }

        // Sent chunked acquisition (item 2). `delta.new_uids` is every
        // still-unacquired in-window UID (ascending). For Sent we take only the
        // newest SENT_ROUND_MESSAGES this round and defer the rest, so a 5,000-
        // message Sent folder fills over many polls instead of one giant round.
        // Flag changes and deletions for already-acquired messages are NOT
        // budgeted — only the new-mail acquisition is chunked. Progress is
        // crash-safe because "still unacquired" is derived from local
        // locations every round: an interrupted round commits nothing (the
        // whole round is one transaction), so it loses nothing and the next
        // round recomputes the same remaining set and acquires nothing twice.
        //
        // The watermark (`backfill_low_uid`) is the lowest UID acquired while
        // the backfill is INCOMPLETE, NULL once complete. "Complete" = after
        // this chunk there is no still-unacquired in-window UID left AND none
        // fell beyond the window unseen. We record it in the SAME round, so the
        // cadence gate above reads an accurate pending flag next poll.
        let mut sent_backfill_write: Option<Option<i64>> = None;
        if is_sent {
            let budget = self.sent_round_budget.unwrap_or(policy::SENT_ROUND_MESSAGES);
            let acquiring_total = delta.new_uids.len();
            if acquiring_total > budget {
                // Keep the NEWEST (highest) `budget` UIDs; the ascending tail
                // is the newest.
                let drop_count = acquiring_total - budget;
                delta.new_uids.drain(0..drop_count);
            }
            // After this chunk, is anything still unacquired in-window? If this
            // round takes every remaining new UID, the backfill is complete.
            let remaining_after_chunk = acquiring_total.saturating_sub(delta.new_uids.len());
            if remaining_after_chunk == 0 {
                // Nothing left to acquire in-window: backfill complete (NULL).
                sent_backfill_write = Some(None);
            } else {
                // Still acquiring: the watermark is the lowest UID acquired SO
                // FAR, i.e. min(previous watermark, lowest UID in this chunk).
                let chunk_low = delta.new_uids.first().copied().map(|u| u as i64);
                // On a UIDVALIDITY reset the old incarnation's watermark is
                // meaningless (locations were dropped, UIDs renumbered), so the
                // backfill restarts from this chunk's low UID.
                let prior = if delta.uidvalidity_reset {
                    None
                } else {
                    self.store.sent_backfill_low_uid(mailbox).map_err(db_err)?
                };
                let low = match (prior, chunk_low) {
                    (Some(p), Some(c)) => Some(p.min(c)),
                    (Some(p), None) => Some(p),
                    (None, c) => c,
                };
                // A pending backfill always has a non-NULL marker so the gate
                // keeps re-running; fall back to UIDNEXT if we somehow have no
                // UID yet (empty chunk but work remains — shouldn't happen).
                sent_backfill_write = Some(Some(low.unwrap_or(server_uidnext.max(1))));
            }
            if delta.beyond_window > 0 {
                log::info!(
                    "imap sync: Sent backfill — {} UID(s) beyond the {}-message window will not be acquired",
                    delta.beyond_window,
                    limits.mailbox_window
                );
            }
            log::info!(
                "imap sync: Sent backfill — acquiring {} this round, {} still pending",
                delta.new_uids.len(),
                remaining_after_chunk
            );
        }

        let mut round = SyncRoundWrite::default();
        round.mailbox_state =
            Some((mailbox.to_string(), server_exists, server_uidnext, new_last_sweep_at, now, epoch));
        // Sent backfill watermark, written in the SAME transaction as this
        // round's acquired chunk so progress is exactly as crash-safe as the
        // locations it accompanies (item 2). `None` for every non-Sent mailbox.
        round.sent_backfill_low_uid = sent_backfill_write;

        // Search-pin releases decided above (TTL-expired pins, or every pin of
        // the old incarnation on a UIDVALIDITY reset), committed in this same
        // round's transaction so a released pin never outlives its decision.
        round.pin_releases.append(&mut pin_releases);

        // On a UIDVALIDITY reset the local UIDs are meaningless: drop every
        // location IN THIS MAILBOX only (bodies survive — keyed by stable id,
        // and other mailboxes' locations and message->thread mappings are
        // untouched). The reset's "new" set is the whole windowed server view.
        let dropped_message_ids: Vec<String> = if delta.uidvalidity_reset {
            round.deletions = local_locations
                .iter()
                .map(|l| (l.mailbox.clone(), l.uidvalidity, l.uid))
                .collect();
            local_locations.iter().map(|l| l.message_id.clone()).collect()
        } else {
            Vec::new()
        };

        let mut threading_inputs: Vec<ThreadingInput> = Vec::new();
        let mut changed_message_ids: Vec<String> = Vec::new();
        // Message ids newly located in this mailbox this round (new mail), used
        // to compute hotness and label transitions.
        let mut new_message_ids: Vec<String> = Vec::new();

        // New mail: identity pass + (for hot mailboxes) body fetch, threading
        // inputs, and location rows. Factored into `ingest_new_uids` so the
        // explicit-UID acquire path (`acquire_uids`, Phase 5 run 3) runs the
        // byte-identical ingest for a search's UID list; `refresh_mailbox_with`
        // keeps calling it with `delta.new_uids`, so the existing sync tests
        // (the contract) are unchanged.
        let ingest = self
            .ingest_new_uids(session, mailbox, server_uidvalidity, &delta.new_uids, makes_hot)
            .await?;
        round.locations.extend(ingest.locations);
        threading_inputs.extend(ingest.threading_inputs);
        changed_message_ids.extend(ingest.changed_message_ids);
        new_message_ids.extend(ingest.new_message_ids);

        // Flag changes: collect an updated location row (not applied yet).
        if !delta.uidvalidity_reset {
            for &uid in &delta.flag_changed_uids {
                if let Some(flags) = server_view.flags_by_uid.get(&uid) {
                    if let Some(message_id) = self
                        .store
                        .location_message_id(mailbox, server_uidvalidity, uid as i64)
                        .map_err(db_err)?
                    {
                        round.locations.push(ImapLocation {
                            mailbox: mailbox.to_string(),
                            uidvalidity: server_uidvalidity,
                            uid: uid as i64,
                            message_id: message_id.clone(),
                            flags_json: serde_json::to_string(flags)
                                .unwrap_or_else(|_| "[]".to_string()),
                            modseq: None,
                        });
                        changed_message_ids.push(message_id);
                    }
                }
            }

            // Deletions: collect the location coordinate to delete.
            for &uid in &delta.deleted_uids {
                if let Some(message_id) = self
                    .store
                    .location_message_id(mailbox, server_uidvalidity, uid as i64)
                    .map_err(db_err)?
                {
                    round
                        .deletions
                        .push((mailbox.to_string(), server_uidvalidity, uid as i64));
                    changed_message_ids.push(message_id);
                }
                // A pinned UID that the delta reports deleted was expunged on
                // the server (a pin never ages out of the window). Release its
                // pin in the same round so a stale pin cannot resurrect it.
                if effective_pins.contains(&uid) {
                    round
                        .pin_releases
                        .push((mailbox.to_string(), server_uidvalidity, uid as i64));
                }
            }
        }

        // Thread the new messages, seeding prior thread state from PERSISTED
        // tokens (never cached bodies), so index-tier messages thread fine.
        let outcome = if threading_inputs.is_empty() {
            threading::ThreadingOutcome::default()
        } else {
            self.ensure_tokens_backfilled()?;
            let batch_tokens: Vec<String> = threading_inputs
                .iter()
                .flat_map(|input| input.token_set())
                .collect();
            let mut thread_state = self.load_thread_state_from_tokens(&batch_tokens)?;
            threading::thread_batch(&mut thread_state, &threading_inputs)
        };

        // Resolve each message's thread id after this round's assignments, so
        // hotness and journaling see the final grouping.
        let mut assignment_of: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (message_id, thread_id) in &outcome.assignments {
            assignment_of.insert(message_id.clone(), thread_id.clone());
        }

        // Changed thread ids: threading effects, plus the threads of any
        // flag-changed / deleted / dropped message.
        let mut changed_threads: std::collections::BTreeSet<String> =
            outcome.changed_threads.iter().cloned().collect();
        for message_id in changed_message_ids.iter().chain(dropped_message_ids.iter()) {
            if let Some(thread) = self.store.thread_of_message(message_id).map_err(db_err)? {
                changed_threads.insert(thread);
            }
            if let Some(thread) = assignment_of.get(message_id) {
                changed_threads.insert(thread.clone());
            }
        }
        for (_, thread_id) in &outcome.assignments {
            changed_threads.insert(thread_id.clone());
        }

        // Hotness (item 5): a thread with a NEW location in INBOX/Sent is hot.
        // Resolve each new message to its (post-assignment) thread id. A merge
        // survivor is carried across aliases by commit_sync_round's resolver.
        let mut hot_threads: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        if makes_hot {
            for message_id in &new_message_ids {
                let thread = match assignment_of.get(message_id) {
                    Some(thread) => Some(thread.clone()),
                    None => self.store.thread_of_message(message_id).map_err(db_err)?,
                };
                if let Some(thread) = thread {
                    hot_threads.insert(thread);
                }
            }
        }

        round.thread_assignments = outcome.assignments;
        round.aliases = outcome.aliases;
        round.message_tokens = threading_inputs
            .iter()
            .map(|input| (input.message_id.clone(), input.token_set()))
            .collect();
        round.hot_threads = hot_threads.into_iter().collect();

        // Journaling (item 5): only HOT threads are reported to the engine. A
        // changed thread that is not hot (and does not become hot this round)
        // is recorded but NOT journaled, so a Trash/Junk-only thread never
        // reaches the engine. A thread made hot THIS round is journaled.
        let newly_hot: std::collections::BTreeSet<&String> = round.hot_threads.iter().collect();
        let mut journaled: Vec<String> = Vec::new();
        for thread_id in &changed_threads {
            let resolved = self.store.resolve_thread_alias(thread_id).map_err(db_err)?;
            let already_hot = self.store.is_thread_hot(&resolved).map_err(db_err)?;
            if already_hot || newly_hot.contains(&resolved) || newly_hot.contains(thread_id) {
                journaled.push(thread_id.clone());
            }
        }
        round.changed_threads = journaled;

        // Idle round: nothing content-changed. Still persist cadence counters
        // (item 4) without bumping the generation. commit_sync_round handles
        // the mailbox_state write on an empty round.
        let generation = self.store.commit_sync_round(&round).map_err(db_err)?;
        self.store.prune_journal().map_err(db_err)?;
        Ok(generation)
    }

    /// Ingest a list of UIDs in one mailbox: for each UID run the SAME identity
    /// pass → (hot: body fetch) → threading-input derivation → location row the
    /// live-sync new-mail loop runs, in UID batches. Returns the accumulated
    /// location rows, threading inputs and message-id lists for the caller to
    /// fold into its round. Does NOT touch the round, the delta or the store's
    /// generation — it is the pure ingest step shared by `refresh_mailbox_with`
    /// (driven by `delta.new_uids`) and `acquire_uids` (driven by a search's
    /// explicit UID list), so both behave byte-identically. `makes_hot` selects
    /// the hot (identity+body) vs index-tier (headers only) path exactly as the
    /// live loop did.
    async fn ingest_new_uids(
        &self,
        session: &mut dyn ImapSession,
        mailbox: &str,
        server_uidvalidity: i64,
        uids: &[u32],
        makes_hot: bool,
    ) -> ProviderResult<IngestOutcome> {
        let mut outcome = IngestOutcome::default();
        for batch in chunk(uids, policy::UID_BATCH_SIZE) {
            let set = uid_set(batch);
            if makes_hot {
                // Hot mailbox: identity pass + body once (deferred location).
                let rows = fetch_identity(session, &set).await?;
                for row in rows {
                    let mut fetcher = SessionBodyFetcher { session };
                    let resolved = crate::provider::imap::fetch::resolve_and_cache_body(
                        &mut fetcher,
                        &self.store,
                        &self.cache,
                        mailbox,
                        server_uidvalidity,
                        &row,
                        (self.now)(),
                    )
                    .await?;
                    let message_id = resolved.message_id;
                    outcome.locations.push(ImapLocation {
                        mailbox: mailbox.to_string(),
                        uidvalidity: server_uidvalidity,
                        uid: row.uid as i64,
                        message_id: message_id.clone(),
                        flags_json: serde_json::to_string(&row.flags)
                            .unwrap_or_else(|_| "[]".to_string()),
                        modseq: None,
                    });
                    let headers = if resolved.body_skipped {
                        crate::provider::imap::rfc822::ThreadingHeaders {
                            message_id: row.inputs.message_id.clone(),
                            ..Default::default()
                        }
                    } else {
                        match self.cache.get(&message_id).map_err(db_err)? {
                            Some(cached) => crate::provider::imap::rfc822::threading_headers_from_raw(&cached.raw),
                            None => crate::provider::imap::rfc822::ThreadingHeaders {
                                message_id: row.inputs.message_id.clone(),
                                ..Default::default()
                            },
                        }
                    };
                    outcome.threading_inputs.push(ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: headers
                            .message_id
                            .or_else(|| row.inputs.message_id.clone()),
                        in_reply_to: headers.in_reply_to,
                        references: headers.references,
                    });
                    outcome.changed_message_ids.push(message_id.clone());
                    outcome.new_message_ids.push(message_id);
                }
            } else {
                // Index-tier mailbox: INDEX identity pass (headers carry the
                // threading triple); NO body fetched. The stable id is derived
                // the same way, so a message in INBOX and in Trash is ONE id.
                let rows = crate::provider::imap::fetch::fetch_identity_index(session, &set).await?;
                for (row, threading) in rows {
                    let message_id = {
                        // Sticky id: an id already recorded for this coordinate
                        // wins; otherwise derive it from the identity inputs.
                        match self
                            .store
                            .location_message_id(mailbox, server_uidvalidity, row.uid as i64)
                            .map_err(db_err)?
                        {
                            Some(existing) => existing,
                            None => crate::provider::imap::identity::derive_message_id(
                                self.store.account_id(),
                                &row.inputs,
                            ),
                        }
                    };
                    outcome.locations.push(ImapLocation {
                        mailbox: mailbox.to_string(),
                        uidvalidity: server_uidvalidity,
                        uid: row.uid as i64,
                        message_id: message_id.clone(),
                        flags_json: serde_json::to_string(&row.flags)
                            .unwrap_or_else(|_| "[]".to_string()),
                        modseq: None,
                    });
                    outcome.threading_inputs.push(ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: threading
                            .message_id
                            .or_else(|| row.inputs.message_id.clone()),
                        in_reply_to: threading.in_reply_to,
                        references: threading.references,
                    });
                    outcome.changed_message_ids.push(message_id.clone());
                    outcome.new_message_ids.push(message_id);
                }
            }
        }
        Ok(outcome)
    }

    /// Acquire an EXPLICIT list of UIDs in one mailbox and PIN them, for the
    /// server-search ingest path (Phase 5 run 3; run 4 is the only caller).
    ///
    /// A search hit is usually OLD — outside the mailbox's newest-N sync window
    /// — so acquiring it the way `refresh_mailbox_with` acquires new mail is not
    /// enough: the next poll's delta would age the out-of-window location out of
    /// an evicting mailbox and delete it. So every UID this acquires is also
    /// PINNED (schema v70): the delta then retains it until the SERVER expunges
    /// it or the pin's TTL releases it. Acquired threads are marked HOT so the
    /// engine ingests them (only hot threads are journaled) and the emptied-
    /// thread grace protects them if a pinned location later vanishes; hotness
    /// adds no INBOX label (labels come from the location's mailbox, not from
    /// hotness), so INBOX semantics stay correct — an Archive hit is labelled
    /// `folder:Archive`, never INBOX, and never appears in `list_inbox`.
    ///
    /// Steps, all in ONE transaction (a failure before commit writes nothing):
    ///   1. EXAMINE (read-only — never SELECT, so acquiring never sets `\Seen`).
    ///   2. Verify the server's UIDVALIDITY matches the caller's expectation
    ///      (the incarnation its UID list is addressed under); on a mismatch
    ///      REFUSE with [`AcquireError::UidValidityMismatch`] rather than
    ///      ingesting under the wrong incarnation.
    ///   3. Skip UIDs already located locally (no duplicate message/location),
    ///      but still PIN them (idempotently refreshing an existing pin's TTL).
    ///   4. Fetch + thread the rest (via the shared `ingest_new_uids`).
    ///   5. Enforce [`policy::MAX_SEARCH_PINS`] per account: pin up to the cap,
    ///      NEWEST (highest UID) first, and REFUSE the over-cap UIDs — reported
    ///      in [`AcquireOutcome::skipped_over_cap`], never silently dropped and
    ///      never evicting an existing pin. A refused UID is NOT acquired at all
    ///      (acquiring an unpinned out-of-window hit would just let it age out
    ///      next poll, which defeats the purpose), so partial acceptance is the
    ///      documented behaviour.
    ///
    /// Returns the affected thread ids (resolved through merge aliases) so the
    /// caller can report them. Bodies may be cached eagerly during the fetch
    /// (the body cache is rebuildable and keyed by stable id), exactly as the
    /// live sync does — only the durable location/thread/pin/hot/journal writes
    /// are deferred to the atomic commit.
    pub(super) async fn acquire_uids(
        &self,
        session: &mut dyn ImapSession,
        entry: &plan::PlanEntry,
        uids: &[u32],
        expected_uidvalidity: i64,
    ) -> Result<AcquireOutcome, AcquireError> {
        let mailbox = entry.mailbox.as_str();

        // 0. Only a mailbox the sync visits can hold a pin: TTL release, flag
        // refresh and expunge detection all run inside that mailbox's sync
        // round. A pin in a mailbox outside the sync cap would never be
        // maintained or released, so refuse before any network work.
        if !entry.synced_now {
            return Err(AcquireError::MailboxNotSynced {
                mailbox: mailbox.to_string(),
            });
        }

        // 1. EXAMINE (read-only).
        let status = session.examine(mailbox).await.map_err(AcquireError::Provider)?;
        let server_uidvalidity = status.uid_validity.unwrap_or(0) as i64;

        // 2. UIDVALIDITY guard: refuse rather than ingest under the wrong
        // incarnation. The caller's UID list is meaningful only under the
        // incarnation it was searched in.
        if server_uidvalidity != expected_uidvalidity {
            return Err(AcquireError::UidValidityMismatch {
                mailbox: mailbox.to_string(),
                expected: expected_uidvalidity,
                found: server_uidvalidity,
            });
        }

        let now = (self.now)();
        // Acquired threads are hot (see the doc comment): the search ingest
        // path always marks hot so the engine ingests the thread and the
        // emptied-thread grace protects it. This is independent of the
        // mailbox's role — an Archive hit is still hot (but unlabelled INBOX).
        let makes_hot = true;
        // Whether to fetch bodies at ingest: a message we will show needs its
        // body, so the acquire path always fetches like a hot mailbox.
        let fetch_bodies = true;

        // 3. Partition the requested UIDs into already-local (skip the fetch,
        // but still pin) and to-fetch. Dedup and sort so "newest first" for the
        // cap is well-defined.
        let mut unique: Vec<u32> = uids.to_vec();
        unique.sort_unstable();
        unique.dedup();

        let mut already_local: Vec<u32> = Vec::new();
        let mut to_fetch: Vec<u32> = Vec::new();
        for &uid in &unique {
            let located = self
                .store
                .location_message_id(mailbox, server_uidvalidity, uid as i64)
                .map_err(|e| AcquireError::Provider(db_err(e)))?
                .is_some();
            if located {
                already_local.push(uid);
            } else {
                to_fetch.push(uid);
            }
        }

        // 5 (cap, applied before the fetch so a refused UID is never fetched).
        // A UID already pinned refreshes in place and does NOT count against
        // new-pin room; count only the requested UIDs that are not already
        // pinned as "new". Accept NEWEST first.
        let mut new_pin_candidates: Vec<u32> = Vec::new();
        let mut refreshing: Vec<u32> = Vec::new();
        for &uid in &unique {
            if self
                .store
                .is_pinned(mailbox, server_uidvalidity, uid as i64)
                .map_err(|e| AcquireError::Provider(db_err(e)))?
            {
                refreshing.push(uid);
            } else {
                new_pin_candidates.push(uid);
            }
        }
        let existing_total = self
            .store
            .search_pin_count()
            .map_err(|e| AcquireError::Provider(db_err(e)))?;
        let cap = policy::cap_search_pins(existing_total, new_pin_candidates.len());
        // Newest-first acceptance: keep the highest `accepted` UIDs among the
        // new candidates; the rest are refused.
        new_pin_candidates.sort_unstable();
        let refused_count = cap.refused;
        let accepted_new: std::collections::BTreeSet<u32> = new_pin_candidates
            .iter()
            .rev()
            .take(cap.accepted)
            .copied()
            .collect();
        // The UIDs this acquisition will PIN: every refresh (already pinned)
        // plus the accepted new ones. A refused new UID is pinned by neither,
        // and is not fetched/located either.
        let pinned_now: std::collections::BTreeSet<u32> = refreshing
            .iter()
            .copied()
            .chain(accepted_new.iter().copied())
            .collect();

        // The UIDs we will actually fetch bodies for: to-fetch UIDs that are
        // being pinned this round (a refused UID is skipped entirely).
        let fetch_set: Vec<u32> = to_fetch
            .iter()
            .copied()
            .filter(|uid| pinned_now.contains(uid))
            .collect();

        // 4. Fetch + thread the to-acquire set via the shared ingest step.
        let ingest = self
            .ingest_new_uids(session, mailbox, server_uidvalidity, &fetch_set, fetch_bodies)
            .await
            .map_err(AcquireError::Provider)?;

        let mut round = SyncRoundWrite {
            locations: ingest.locations,
            ..Default::default()
        };

        // Pin every UID we are keeping (refresh or newly accepted), at `now`,
        // but ONLY if it has a location after this round: an already-local
        // UID, or one the fetch actually returned. A UID expunged between the
        // caller's SEARCH and this FETCH yields no location, and a pin without
        // a location is never released by the delta (which only deletes local
        // UIDs), so it would sit in the cap until its TTL.
        let located_after: std::collections::BTreeSet<u32> = already_local
            .iter()
            .copied()
            .chain(round.locations.iter().map(|l| l.uid as u32))
            .collect();
        for &uid in pinned_now.intersection(&located_after) {
            round
                .pin_upserts
                .push((mailbox.to_string(), server_uidvalidity, uid as i64, now));
        }

        // Thread the newly-fetched messages, seeding prior thread state from
        // PERSISTED tokens, exactly as the live loop does.
        let outcome = if ingest.threading_inputs.is_empty() {
            threading::ThreadingOutcome::default()
        } else {
            self.ensure_tokens_backfilled().map_err(AcquireError::Provider)?;
            let batch_tokens: Vec<String> = ingest
                .threading_inputs
                .iter()
                .flat_map(|input| input.token_set())
                .collect();
            let mut thread_state = self
                .load_thread_state_from_tokens(&batch_tokens)
                .map_err(AcquireError::Provider)?;
            threading::thread_batch(&mut thread_state, &ingest.threading_inputs)
        };

        let mut assignment_of: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (message_id, thread_id) in &outcome.assignments {
            assignment_of.insert(message_id.clone(), thread_id.clone());
        }

        // Affected threads: every acquired message's thread, plus every
        // already-local pinned UID's thread (so re-pinning an existing hit also
        // keeps/affirms its thread hot), resolved through aliases.
        let mut affected_threads: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for message_id in &ingest.new_message_ids {
            let thread = match assignment_of.get(message_id) {
                Some(t) => Some(t.clone()),
                None => self
                    .store
                    .thread_of_message(message_id)
                    .map_err(|e| AcquireError::Provider(db_err(e)))?,
            };
            if let Some(t) = thread {
                affected_threads.insert(t);
            }
        }
        // Already-local pinned UIDs: resolve their message -> thread so a
        // re-pin affirms hotness and journals the thread.
        for &uid in &already_local {
            if !pinned_now.contains(&uid) {
                continue;
            }
            if let Some(message_id) = self
                .store
                .location_message_id(mailbox, server_uidvalidity, uid as i64)
                .map_err(|e| AcquireError::Provider(db_err(e)))?
            {
                if let Some(t) = self
                    .store
                    .thread_of_message(&message_id)
                    .map_err(|e| AcquireError::Provider(db_err(e)))?
                {
                    affected_threads.insert(t);
                }
            }
        }
        for (_, thread_id) in &outcome.assignments {
            affected_threads.insert(thread_id.clone());
        }

        round.thread_assignments = outcome.assignments;
        round.aliases = outcome.aliases;
        round.message_tokens = ingest
            .threading_inputs
            .iter()
            .map(|input| (input.message_id.clone(), input.token_set()))
            .collect();

        // Mark every affected thread hot (the acquire decision) and journal it
        // so the engine ingests it. Resolve through aliases to the survivor.
        let mut hot_threads: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        if makes_hot {
            for thread_id in &affected_threads {
                let resolved = self
                    .store
                    .resolve_thread_alias(thread_id)
                    .map_err(|e| AcquireError::Provider(db_err(e)))?;
                hot_threads.insert(resolved);
            }
        }
        round.hot_threads = hot_threads.into_iter().collect();
        // Journal every affected thread (resolved) so the engine re-ingests the
        // acquired hit. Unlike the sync loop, the acquire path journals ALL
        // affected threads: they are all made hot here, so all are reportable.
        let mut journaled: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for thread_id in &affected_threads {
            journaled.insert(thread_id.clone());
            let resolved = self
                .store
                .resolve_thread_alias(thread_id)
                .map_err(|e| AcquireError::Provider(db_err(e)))?;
            journaled.insert(resolved);
        }
        round.changed_threads = journaled.into_iter().collect();

        let generation = self
            .store
            .commit_sync_round(&round)
            .map_err(|e| AcquireError::Provider(db_err(e)))?;
        self.store
            .prune_journal()
            .map_err(|e| AcquireError::Provider(db_err(e)))?;

        // The affected thread ids (resolved) for the caller to report.
        let mut affected: Vec<String> = Vec::new();
        for thread_id in &affected_threads {
            let resolved = self
                .store
                .resolve_thread_alias(thread_id)
                .map_err(|e| AcquireError::Provider(db_err(e)))?;
            if !affected.contains(&resolved) {
                affected.push(resolved);
            }
        }
        affected.sort();

        Ok(AcquireOutcome {
            generation,
            affected_threads: affected,
            skipped_over_cap: refused_count,
        })
    }

    fn local_view(
        &self,
        uidvalidity: Option<i64>,
        locations: &[ImapLocation],
    ) -> crate::provider::imap::delta::LocalMailboxView {
        let mut flags_by_uid = std::collections::BTreeMap::new();
        for location in locations {
            let flags: Vec<String> =
                serde_json::from_str(&location.flags_json).unwrap_or_default();
            flags_by_uid.insert(location.uid as u32, flags);
        }
        crate::provider::imap::delta::LocalMailboxView {
            uidvalidity,
            flags_by_uid,
        }
    }

    /// Fetch FLAGS for a UID set, batched, returning uid -> wire flags. Uses
    /// the FLAGS-ONLY fetch item (no body, no headers, no size), so the
    /// per-poll flag sweep over the window is cheap even at the ceiling.
    pub(super) async fn fetch_flags(
        &self,
        session: &mut dyn ImapSession,
        uids: &[u32],
    ) -> ProviderResult<std::collections::BTreeMap<u32, Vec<String>>> {
        let mut out = std::collections::BTreeMap::new();
        for batch in chunk(uids, policy::UID_BATCH_SIZE) {
            if batch.is_empty() {
                continue;
            }
            let set = uid_set(batch);
            for (uid, flags) in crate::provider::imap::fetch::fetch_flags_only(session, &set).await? {
                out.insert(uid, flags);
            }
        }
        Ok(out)
    }

    /// Seed the threader's prior state from PERSISTED tokens — never from
    /// cached bodies. Given the tokens the incoming batch carries, the store
    /// finds existing messages that share any token, resolves their threads
    /// through aliases, and returns each seeded thread's full token set plus
    /// its persisted creation generation. Only the threads the batch can
    /// actually touch are seeded, so cost scales with the batch, not with all
    /// mail, and an index-tier message with no cached body threads fine.
    ///
    /// `thread_state_loads` still counts a real load here (an idle round
    /// skips it, as before), so the 75f2704 counter test stays meaningful.
    pub(super) fn load_thread_state_from_tokens(&self, batch_tokens: &[String]) -> ProviderResult<ThreadState> {
        self.thread_state_loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut state = ThreadState::new();
        let seeded = self.store.seed_state_for_tokens(batch_tokens).map_err(db_err)?;
        for (thread_id, (created_generation, tokens)) in seeded {
            state.seed(&thread_id, created_generation, &tokens);
        }
        Ok(state)
    }

    /// Run the one-time, idempotent, bounded-batch, crash-resumable upgrade
    /// backfill that fills `imap_message_tokens` for threads that predate v59
    /// (a database with threaded mail from 0.94.x has no token rows). This is
    /// the ONLY place that still parses cached bodies. A message with no cached
    /// body gets only its stable-id token (matching c77fa3f's absent-body
    /// behaviour). It runs just before the first threading that needs tokens;
    /// a new account is already marked done, so this returns immediately.
    ///
    /// Each message's tokens are written in their own transaction and the
    /// completion flag is set only after the queue drains, so a crash mid-way
    /// leaves every finished message whole and the next run resumes from the
    /// messages still missing tokens.
    pub(super) fn ensure_tokens_backfilled(&self) -> ProviderResult<()> {
        if self.store.tokens_backfilled().map_err(db_err)? {
            return Ok(());
        }
        use crate::provider::imap::identity::normalize_message_id;
        loop {
            let batch = self
                .store
                .messages_missing_tokens(policy::TOKEN_BACKFILL_BATCH_SIZE)
                .map_err(db_err)?;
            if batch.is_empty() {
                break;
            }
            for message_id in &batch {
                // Reconstruct this message's token set from its cached body.
                // No body (expunged, oversize-skipped, index-tier) => only the
                // stable-id anchor, exactly as c77fa3f seeded such a message.
                let tokens = match self.cache.get(message_id).map_err(db_err)? {
                    Some(cached) => {
                        let headers = crate::provider::imap::rfc822::threading_headers_from_raw(&cached.raw);
                        let input = ThreadingInput {
                            message_id: message_id.clone(),
                            message_id_header: headers
                                .message_id
                                .as_deref()
                                .map(normalize_message_id)
                                .or_else(|| Some(message_id.clone())),
                            in_reply_to: headers.in_reply_to,
                            references: headers.references,
                        };
                        input.token_set()
                    }
                    None => vec![message_id.clone()],
                };
                self.store
                    .write_message_tokens(message_id, &tokens)
                    .map_err(db_err)?;
            }
            // A full batch that returned fewer than the limit means the queue
            // is drained; loop once more only if it was exactly the limit.
            if batch.len() < policy::TOKEN_BACKFILL_BATCH_SIZE {
                break;
            }
        }
        self.store.mark_tokens_backfilled().map_err(db_err)?;
        Ok(())
    }

    /// TEST-ONLY ORACLE: the pre-5b1 body-parsing seeder, kept verbatim so the
    /// equivalence tests can assert the token loader threads identically. It
    /// reads and parses every cached body (what 5b-1 removed from the hot
    /// path); it must never be called from production.
    #[cfg(test)]
    pub(super) fn load_thread_state(&self) -> ProviderResult<ThreadState> {
        use crate::provider::imap::identity::normalize_message_id;
        let mut state = ThreadState::new();
        let rows = self.store.all_message_threads().map_err(db_err)?;
        let mut by_thread: std::collections::BTreeMap<String, (u64, Vec<String>)> =
            std::collections::BTreeMap::new();
        for (message_id, thread_id, created) in rows {
            let entry = by_thread
                .entry(
                    self.store
                        .resolve_thread_alias(&thread_id)
                        .map_err(db_err)?,
                )
                .or_insert((created, Vec::new()));
            entry.0 = entry.0.min(created);
            entry.1.push(message_id);
        }
        for (thread_id, (created_generation, message_ids)) in by_thread {
            let mut tokens: Vec<String> = Vec::new();
            for message_id in &message_ids {
                tokens.push(message_id.clone());
                if let Some(cached) = self.cache.get(message_id).map_err(db_err)? {
                    let headers = crate::provider::imap::rfc822::threading_headers_from_raw(&cached.raw);
                    let input = ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: headers.message_id.clone(),
                        in_reply_to: headers.in_reply_to,
                        references: headers.references,
                    };
                    tokens.extend(input.reference_tokens().0);
                    if let Some(raw_id) = headers.message_id {
                        let normalized = normalize_message_id(&raw_id);
                        if !normalized.is_empty() {
                            tokens.push(normalized);
                        }
                    }
                }
            }
            state.seed(&thread_id, created_generation, &tokens);
        }
        Ok(state)
    }
}

#[async_trait]
impl MailSync for ImapProvider {
    fn thread_aliases(&self) -> ProviderResult<Vec<(String, String)>> {
        self.store.thread_aliases().map_err(db_err)
    }

    async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
        let refreshed = self.refresh_all().await?;
        // The engine seeds full recovery from list_inbox alone. Re-journal
        // every other hot thread, including acquired Sent mail from an earlier
        // interrupted baseline and threads whose last location disappeared.
        // Rebuild from durable hotness rather than old journal rows, which may
        // already have been pruned. Repeating this after a crash is harmless.
        let inbox: std::collections::BTreeSet<String> = self
            .store
            .thread_ids_in_mailbox(INBOX)
            .map_err(db_err)?
            .into_iter()
            .collect();
        let changed_threads = self
            .store
            .hot_thread_ids()
            .map_err(db_err)?
            .into_iter()
            .filter(|thread| !inbox.contains(thread))
            .collect();
        self.store
            .commit_sync_round(&SyncRoundWrite {
                changed_threads,
                ..Default::default()
            })
            .map_err(db_err)?;
        // The engine's follow-up incremental pass consumes the replay above.
        Ok(SyncCursor::from_generation(refreshed.last))
    }

    async fn poll(&self, cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
        let Some(polled) = cursor.generation() else {
            // Not a generation cursor (e.g. a stale Gmail-style string): the
            // engine answers InvalidCursor with a full resync.
            return Err(ProviderError::InvalidCursor);
        };
        let before = self.store.generation().map_err(db_err)?;
        // A cursor newer than the store's generation (store reset behind the
        // engine) or older than retention cannot be answered completely.
        if !policy::journal_cursor_is_answerable(polled, before) {
            return Err(ProviderError::InvalidCursor);
        }
        // (a) refresh all synced mailboxes (INBOX first); (b) return threads
        // journalled since `polled`; (c) new cursor = current generation.
        let refreshed = self.refresh_all().await?;
        let changed = self.store.journal_since(polled).map_err(db_err)?;
        Ok(SyncBatch {
            changed_threads: changed,
            cursor: SyncCursor::from_generation(refreshed.last),
            more: false,
        })
    }

    async fn list_inbox(&self, _page: Option<&str>) -> ProviderResult<ThreadPage> {
        // A single page from the local store: thread ids with an INBOX
        // location. next = None (no server paging this slice).
        let thread_ids = self.store.thread_ids_in_mailbox(INBOX).map_err(db_err)?;
        Ok(ThreadPage {
            thread_ids,
            next: None,
        })
    }

    async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>> {
        // Resolve an aliased (merged-away) id to its survivor.
        let thread_id = self.store.resolve_thread_alias(id).map_err(db_err)?;
        let message_ids = self.store.messages_in_thread(&thread_id).map_err(db_err)?;

        // Does this thread have ANY live location right now? A message row can
        // outlive its last location (the location was expunged / the message
        // was moved to a mailbox not swept this poll), so "has messages" is not
        // "has a location". Zero locations is the trigger for the emptied-hot-
        // thread grace (Slice 5b-2 item 4).
        let mut has_any_location = false;
        for message_id in &message_ids {
            if !self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?
                .is_empty()
            {
                has_any_location = true;
                break;
            }
        }

        if !has_any_location {
            // The thread has no locations. If it is (or ever was) HOT, do NOT
            // report NotFound immediately: another client may have moved a hot
            // message out of INBOX into a mailbox not swept this poll, so the
            // thread momentarily has zero locations but regains one as soon as
            // the destination is visited. Return `Ok(empty)` during the grace
            // (the engine's ingest_threads applies nothing and leaves the local
            // thread, its tasks and labels untouched), and `NotFound` only once
            // EMPTIED_THREAD_GRACE_WALKS completed COVERAGE EPOCHS have passed
            // (an epoch = every synced mailbox visited across one or more polls).
            if self.store.is_thread_hot(&thread_id).map_err(db_err)? {
                let walks = self.store.complete_walks().map_err(db_err)?;
                match self.store.hot_thread_emptied_at_walk(&thread_id).map_err(db_err)? {
                    None => {
                        // First observation of emptiness: start the grace
                        // clock at the current completed-epoch count and keep
                        // the local copy for now.
                        self.store.mark_hot_thread_emptied(&thread_id, walks).map_err(db_err)?;
                        return Ok(Vec::new());
                    }
                    Some(emptied_at) => {
                        if policy::emptied_grace_expired(emptied_at, walks) {
                            // Grace spent: the thread really is gone.
                            return Err(ProviderError::NotFound);
                        }
                        // Still within the grace window.
                        return Ok(Vec::new());
                    }
                }
            }
            // Not a hot thread (cold threads are never journaled, so the engine
            // never asks about one that is gone — but keep the original
            // contract): the engine deletes the local copy.
            return Err(ProviderError::NotFound);
        }

        // The thread regained (or still has) a location: clear any emptied
        // marker so a future emptiness restarts the grace from scratch.
        self.store.clear_hot_thread_emptied(&thread_id).map_err(db_err)?;

        let plan = self.build_sync_plan()?;

        // Thread-wide union of DYNAMIC (`folder:` / `lf:`) label ids across
        // every message's every location (fact 1). The engine builds a thread's
        // non-system labels from the ROOT message only, so a `folder:`/`lf:`
        // label that lives on a reply (or on a message whose only location is a
        // user folder) would be dropped unless EVERY returned message carries
        // it. We gather the union here and append it to each message below.
        // System location ids (INBOX/SENT/SPAM/TRASH) are per-copy, NOT in this
        // union — they are unioned over all messages by the engine already.
        let mut thread_dynamic: Vec<String> = Vec::new();
        for message_id in &message_ids {
            for location in self.store.locations_for_message(message_id).map_err(db_err)? {
                for id in plan.location_label_ids_for_mailbox(&location.mailbox) {
                    if (id.starts_with("folder:") || id.starts_with("lf:"))
                        && !thread_dynamic.contains(&id)
                    {
                        thread_dynamic.push(id);
                    }
                }
            }
        }
        thread_dynamic.sort();

        let mut messages = Vec::new();
        for message_id in &message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            if locations.is_empty() {
                // The message's last location was expunged; skip it.
                continue;
            }
            // Labels: the union across every copy, each classified by the
            // mailbox's RESOLVED location (never an ad-hoc name parse), then
            // the thread-wide dynamic union appended so every message carries
            // the thread's `folder:`/`lf:` labels (fact 1).
            let mut per_copy: Vec<Vec<String>> = locations
                .iter()
                .map(|location| {
                    let flags: Vec<String> =
                        serde_json::from_str(&location.flags_json).unwrap_or_default();
                    labels_for_location_ids(
                        &plan.location_label_ids_for_mailbox(&location.mailbox),
                        &flags,
                    )
                })
                .collect();
            per_copy.push(thread_dynamic.clone());
            let label_ids = merge_label_sets(&per_copy);

            // Body: cache first, else fetch from the best live location.
            let raw = match self.cache.get(message_id).map_err(db_err)? {
                Some(cached) => cached.raw,
                None => {
                    let mut session = self.open_session().await?;
                    let fetched = self.fetch_body_from_server(session.as_mut(), message_id).await;
                    let _ = session.logout().await;
                    match fetched {
                        Ok(Some(raw)) => raw,
                        // No body on the server (expunged between sync and read).
                        Ok(None) => continue,
                        // Oversize/unfetchable body: skip this message with a
                        // reason, do NOT fail the whole thread (design rule /
                        // SLICE5A_FIXES item 2). Only transport/auth propagates.
                        Err(ProviderError::PermanentClientRejection(reason)) => {
                            log::warn!(
                                "imap fetch_thread: skipping {message_id} (oversize body): {reason}"
                            );
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
            };

            match to_raw_message(&raw, message_id, label_ids) {
                Ok(mut message) => {
                    // Point the message at the surviving thread id.
                    message.thread_id = thread_id.clone();
                    messages.push(message);
                }
                Err(ProviderError::PermanentClientRejection(reason)) => {
                    // Per-message oversize: skip this message with a recorded
                    // reason, do NOT fail the whole thread (design rule).
                    log::warn!("imap fetch_thread: skipping {message_id}: {reason}");
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        if messages.is_empty() {
            return Err(ProviderError::NotFound);
        }
        Ok(messages)
    }

    /// Server-side search (Phase 5 run 4). Thin wrapper over
    /// [`ImapProvider::search_detailed`]: it discards the rich result (partial
    /// reason, skipped/no-match notices, unsynced mailboxes, over-cap count —
    /// run 5 will surface those) and returns only the thread ids as a
    /// [`ThreadPage`]. `next` is always `None`: an IMAP search returns one
    /// bounded, deduplicated page (budget + deadline bound it), so there is no
    /// server paging token to hand back. A `Skipped` or `NoServerMatches`
    /// render returns an EMPTY page (never an error), so `search_and_ingest_
    /// missing` and the Sent backfill simply ingest nothing and move on.
    async fn search(&self, query: &str, page: Option<&str>) -> ProviderResult<ThreadPage> {
        let result = self.search_detailed(query, page).await?;
        Ok(ThreadPage {
            thread_ids: result.thread_ids,
            next: None,
        })
    }

    /// IMAP's report-carrying search (Phase 5 run 5): run the rich
    /// [`ImapProvider::search_detailed`] and map its outcome to the
    /// provider-neutral [`SearchReport`] the UI words, WITHOUT the caller ever
    /// branching on provider kind. A provider error still propagates as `Err`
    /// (the UI's existing "unavailable" state), unchanged. The thread-id page
    /// is byte-identical to [`MailSync::search`]'s (same ids, `next: None`).
    async fn search_with_report(
        &self,
        query: &str,
        page: Option<&str>,
    ) -> ProviderResult<(ThreadPage, SearchReport)> {
        let result = self.search_detailed(query, page).await?;
        let report = imap_search_report(&result);
        let page = ThreadPage {
            thread_ids: result.thread_ids,
            next: None,
        };
        Ok((page, report))
    }
}
