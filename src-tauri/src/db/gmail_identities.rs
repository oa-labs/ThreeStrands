//! Server-owned Gmail sending addresses, cached only on this installation.
//! They are never editable through settings transfer or replicated sync.
use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::provider::SendingIdentity;
use rusqlite::{params, OptionalExtension};

pub(crate) const REFRESH_INTERVAL_MS: i64 = 15 * 60 * 1000;

pub(super) fn cached_gmail_identities(
    connection: &rusqlite::Connection,
    account: &str,
) -> DbResult<Option<Vec<SendingIdentity>>> {
    let json: Option<String> = connection.query_row(
        "SELECT g.identities_json FROM gmail_sending_identities g JOIN accounts a ON a.email=g.account_id WHERE g.account_id=?1 AND a.provider='gmail'", [account], |row| row.get(0)).optional()?;
    let Some(json) = json else {
        return Ok(None);
    };
    match serde_json::from_str::<Vec<SendingIdentity>>(&json) {
        Ok(identities)
            if identities
                .first()
                .is_some_and(|identity| identity.address.eq_ignore_ascii_case(account)) =>
        {
            Ok(Some(identities))
        }
        _ => {
            log::warn!("Discarding an unreadable Gmail sending-address cache for {account}");
            Ok(None)
        }
    }
}

impl Database {
    pub(crate) fn gmail_identities_refresh_due(&self, account: &str, at: i64) -> DbResult<bool> {
        self.with_connection(|connection| {
            let last: Option<i64> = connection
                .query_row(
                    "SELECT refreshed_at FROM gmail_sending_identities WHERE account_id=?1",
                    [account],
                    |row| row.get(0),
                )
                .optional()?;
            if cached_gmail_identities(connection, account)?.is_none() {
                return Ok(true);
            }
            Ok(last.is_none_or(|last| at < last || at.saturating_sub(last) >= REFRESH_INTERVAL_MS))
        })
    }

    pub(crate) fn store_gmail_sending_identities(
        &self,
        account: &str,
        identities: &[SendingIdentity],
        at: i64,
    ) -> DbResult<bool> {
        self.require_local_gmail_account(account)?;
        if identities.is_empty()
            || !identities[0].address.eq_ignore_ascii_case(account)
            || identities
                .iter()
                .filter(|identity| identity.is_default)
                .count()
                > 1
        {
            return Err(DatabaseError::invalid(
                "Gmail returned an invalid sending address list",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for identity in identities {
            crate::provider::imap::validate_envelope_address(&identity.address)
                .map_err(|error| DatabaseError::invalid(error.to_string()))?;
            if !seen.insert(identity.address.to_ascii_lowercase())
                || identity
                    .display_name
                    .as_deref()
                    .is_some_and(|name| name.contains(['\r', '\n', '\0']))
            {
                return Err(DatabaseError::invalid(
                    "Gmail returned an invalid sending identity",
                ));
            }
            if let Some(reply_to) = &identity.reply_to {
                crate::provider::imap::validate_envelope_address(reply_to)
                    .map_err(|error| DatabaseError::invalid(error.to_string()))?;
            }
        }
        self.with_transaction(|tx| {
            let before = cached_gmail_identities(tx, account)?;
            tx.execute("INSERT INTO gmail_sending_identities(account_id, identities_json, refreshed_at) VALUES (?1,?2,?3)
                ON CONFLICT(account_id) DO UPDATE SET identities_json=excluded.identities_json, refreshed_at=excluded.refreshed_at",
                params![account, serde_json::to_string(identities).map_err(serialization_error)?, at])?;
            Ok(before.as_deref()!=Some(identities))
        })
    }
}

#[cfg(test)]
#[path = "tests/gmail_identities.rs"]
mod tests;
