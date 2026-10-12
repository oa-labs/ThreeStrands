//! Local label-catalog cache: each account's label NAME -> label ID mapping.
//!
//! `list_labels` resolves a provider's catalog live (a network request for
//! Gmail), and until this cache existed the only copy of the result was the
//! frontend's `useLabelCatalog` state — the database search layer had no way
//! to turn a label NAME (`label:work`, `in:Clients/Acme`) into the opaque IDs
//! stored in `threads.labels_json` (Gmail user ids like `Label_18`; IMAP ids
//! like `lf:<name>` / `folder:<name>` / system ids like `INBOX`).
//!
//! This module is that missing piece and nothing else. It keeps a small LOCAL
//! cache of each account's catalog, kept fresh by best-effort write-through
//! from `list_labels` and the `SyncService` label mutations. It is a
//! rebuildable cache: it is deliberately NOT part of the encrypted settings
//! export and NOT replicated by sync (losing it only forces a re-fetch on the
//! next `list_labels`). Run 2b-2 will add the `label:` / `in:` search filters
//! that consume [`Database::label_ids_named`]; this run changes no search
//! behaviour.

use super::{Database, DbResult};
use crate::models::Label;
use rusqlite::params;

/// The most labels cached for one account. A hostile or buggy server could
/// return an unbounded catalog; this bounds the local cache's size and the
/// cost of the per-change write-through.
///
/// This lives here, in the module that OWNS and ENFORCES the cache, rather
/// than in a provider or in the frontend `emailRenderingPolicy.ts` (which
/// governs rendering limits, not local-cache sizing): the limit is a property
/// of this cache alone, it is provider-neutral, and the one place that checks
/// it ([`check_label_catalog_size`]) and the tests that pin below/exact/above
/// behaviour sit beside it.
pub(crate) const MAX_LABELS_PER_ACCOUNT: usize = 10_000;

/// Why a label listing was not cached. Returned by
/// [`check_label_catalog_size`] so callers handle the over-limit case by TYPE
/// rather than by inspecting a count, and so the write-through can keep the
/// previous cache untouched instead of truncating.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LabelCatalogRejection {
    /// The listing holds more than [`MAX_LABELS_PER_ACCOUNT`] labels. The
    /// payload is the actual count, for the warning log.
    TooMany(usize),
}

/// Decides whether a listing is small enough to cache. `Ok(())` means cache
/// it; `Err` names why not. Pure and side-effect free so it is trivially
/// testable at the below/exact/above boundary.
pub(crate) fn check_label_catalog_size(labels: &[Label]) -> Result<(), LabelCatalogRejection> {
    if labels.len() > MAX_LABELS_PER_ACCOUNT {
        Err(LabelCatalogRejection::TooMany(labels.len()))
    } else {
        Ok(())
    }
}

/// Best-effort write-through for a SUCCESSFUL provider label listing.
///
/// The sole caller today is the `list_labels` command, after it has the
/// provider's result in hand. It NEVER fails the user's action: an over-limit
/// listing or a database error is logged and swallowed, leaving the previous
/// cache untouched, and the command still returns the full listing to the
/// caller. A FAILED listing must not reach here at all — the command only
/// calls this on success, so a provider error leaves the cache alone.
pub(crate) fn cache_label_listing(database: &Database, account_id: &str, labels: &[Label]) {
    if let Err(LabelCatalogRejection::TooMany(count)) = check_label_catalog_size(labels) {
        log::warn!(
            "label cache: {account_id} returned {count} labels (over {MAX_LABELS_PER_ACCOUNT}); keeping previous cache"
        );
        return;
    }
    if let Err(error) = database.replace_account_labels(account_id, labels) {
        log::warn!("label cache: could not cache {account_id} label listing: {error}");
    }
}

impl Database {
    /// Replaces an account's WHOLE cached catalog with exactly `labels`, in
    /// one transaction. Labels no longer listed are removed. Over-limit input
    /// is refused WITHOUT touching the existing cache (the caller logs); the
    /// limit is checked here too so the invariant holds for every caller, not
    /// only the best-effort [`cache_label_listing`] path.
    pub fn replace_account_labels(&self, account_id: &str, labels: &[Label]) -> DbResult<()> {
        if let Err(LabelCatalogRejection::TooMany(count)) = check_label_catalog_size(labels) {
            return Err(super::DatabaseError::invalid(format!(
                "label catalog of {count} exceeds the per-account cache limit of {MAX_LABELS_PER_ACCOUNT}"
            )));
        }
        self.with_transaction(|transaction| {
            transaction.execute(
                "DELETE FROM account_labels WHERE account_id = ?1",
                [account_id],
            )?;
            for label in labels {
                transaction.execute(
                    "INSERT OR REPLACE INTO account_labels(account_id, label_id, name, kind)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![account_id, label.id, label.name, label.kind],
                )?;
            }
            Ok(())
        })
    }

    /// Caches or updates a single label — the write-through for a successful
    /// `SyncService::create_label` / `update_label`.
    pub fn upsert_account_label(&self, account_id: &str, label: &Label) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO account_labels(account_id, label_id, name, kind)
                 VALUES (?1, ?2, ?3, ?4)",
                params![account_id, label.id, label.name, label.kind],
            )?;
            Ok(())
        })
    }

    /// Drops a single cached label — the write-through for a successful
    /// `SyncService::delete_label`.
    pub fn delete_account_label(&self, account_id: &str, label_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM account_labels WHERE account_id = ?1 AND label_id = ?2",
                params![account_id, label_id],
            )?;
            Ok(())
        })
    }

    /// The cached label IDs whose `name` equals `name`, matched
    /// case-insensitively. Several labels may share a name, so this returns a
    /// list, sorted for deterministic output.
    ///
    /// Case-insensitivity is SQLite's `COLLATE NOCASE`: ASCII-only folding,
    /// the SAME limitation the structured search layer has. The comparison is
    /// `=`/NOCASE, NOT `LIKE`, so SQL wildcards (`%`, `_`) and quotes in
    /// `name` are matched literally rather than as patterns.
    ///
    /// This is the lookup run 2b-2 will call to resolve `label:` / `in:`.
    // First production caller lands in run 2b-2 (`label:` / `in:` search);
    // until then this is exercised only by this module's tests.
    #[allow(dead_code)]
    pub fn label_ids_named(&self, account_id: &str, name: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT label_id FROM account_labels
                 WHERE account_id = ?1 AND name = ?2 COLLATE NOCASE
                 ORDER BY label_id",
            )?;
            let rows = statement.query_map(params![account_id, name], |row| row.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

#[cfg(test)]
#[path = "tests/labels.rs"]
mod tests;
