//! Account persistence.

use super::*;

impl Database {
    /// The account the compose pipeline defaults to and the legacy
    /// single-account commands (`google_auth_status`, `connect_google`,
    /// `disconnect_google`) act on: the first row in `accounts`, or the
    /// pre-connect placeholder key before anything has been connected.
    ///
    /// Derived rather than cached so it cannot drift from the account
    /// catalog — this is the same rule startup used when it built a distinct
    /// "primary" credential from `list_accounts().next()`.
    pub fn primary_account_id(&self) -> String {
        self.list_accounts()
            .ok()
            .and_then(|accounts| accounts.into_iter().next())
            .map(|account| account.email)
            .unwrap_or_else(|| crate::auth::LEGACY_KEY.to_string())
    }

    pub fn list_accounts(&self) -> DbResult<Vec<Account>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT email, display_name, color, status, provider, sort_order, connected_at, last_synced_at
                 FROM accounts ORDER BY sort_order",
            )?;
            let rows = statement.query_map([], account_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Ensures an account exists and folds pre-multi-account state onto it.
    ///
    /// `provider` is the service the account just authorized through, so
    /// it is recorded on a new row and kept current on an existing one.
    pub fn adopt_mail_account(
        &self,
        email: &str,
        provider: MailProviderKind,
    ) -> DbResult<Account> {
        self.with_transaction(|transaction| {
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE email = ?1)",
                [email],
                |row| row.get(0),
            )?;
            if exists {
                transaction.execute(
                    "UPDATE accounts SET status = 'connected', provider = ?2 WHERE email = ?1",
                    params![email, provider.as_str()],
                )?;
            } else {
                let sort_order: i64 = transaction.query_row(
                    "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM accounts",
                    [],
                    |row| row.get(0),
                )?;
                let color = ACCOUNT_COLORS[(sort_order as usize) % ACCOUNT_COLORS.len()];
                transaction.execute(
                    "INSERT INTO accounts(email, color, status, provider, sort_order, connected_at)
                     VALUES (?1, ?2, 'connected', ?3, ?4, ?5)",
                    params![email, color, provider.as_str(), sort_order, Utc::now().to_rfc3339()],
                )?;
            }
            for table in ["mutations", "threads", "triage_events"] {
                transaction.execute(
                    &format!("UPDATE {table} SET account_id = ?1 WHERE account_id = 'default'"),
                    [email],
                )?;
            }
            // Older builds recreated the legacy placeholder row on every
            // launch. If this account already has real sync state, discard
            // only that placeholder; otherwise rekey it so first-connect
            // cursors and errors retain their original behavior.
            transaction.execute(
                "DELETE FROM sync_state
                 WHERE account_id = 'default'
                   AND account_id <> ?1
                   AND EXISTS (
                       SELECT 1 FROM sync_state AS existing
                       WHERE existing.account_id = ?1
                   )",
                [email],
            )?;
            transaction.execute(
                "UPDATE sync_state SET account_id = ?1 WHERE account_id = 'default'",
                [email],
            )?;
            transaction.execute(
                "INSERT OR IGNORE INTO sync_state(account_id) VALUES (?1)",
                [email],
            )?;
            Ok(())
        })?;
        self.get_account(email)?
            .ok_or_else(|| "Account not found".into())
    }

    /// Adopts a Gmail account — the shorthand most tests want.
    #[cfg(test)]
    pub fn adopt_account(&self, email: &str) -> DbResult<Account> {
        self.adopt_mail_account(email, MailProviderKind::Gmail)
    }

    pub fn get_account(&self, email: &str) -> DbResult<Option<Account>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT email, display_name, color, status, provider, sort_order, connected_at, last_synced_at
                     FROM accounts WHERE email = ?1",
                    [email],
                    account_from_row,
                )
                .optional()?)
        })
    }

    /// Atomically records that an account's OAuth grant is unusable and
    /// releases any in-flight mutations back to the durable queue. Claiming
    /// is status-gated, so those mutations remain paused until reconnecting
    /// changes the account back to `connected`.
    pub fn mark_account_needs_reauth(&self, email: &str, error: &str) -> DbResult<()> {
        self.with_transaction(|transaction| {
            transaction.execute(
                "UPDATE accounts SET status = 'needs_reauth' WHERE email = ?1",
                [email],
            )?;
            transaction.execute(
                "UPDATE mutations
                 SET state = 'pending', last_error = ?2, next_attempt_at = NULL
                 WHERE account_id = ?1 AND state = 'running'",
                params![email, error],
            )?;
            transaction.execute(
                "UPDATE sync_state SET last_error = ?2 WHERE account_id = ?1",
                params![email, error],
            )?;
            Ok(())
        })
    }

    pub fn account_needs_reauth(&self, email: &str) -> DbResult<bool> {
        self.with_connection(|connection| {
            let status: Option<bool> = connection
                .query_row(
                    "SELECT status = 'needs_reauth' FROM accounts WHERE email = ?1",
                    [email],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(status.unwrap_or(false))
        })
    }

    /// Wipes every trace of an account: credentials are revoked by the
    /// caller only after this commits, so a failure here leaves the account
    /// fully intact rather than stripped of credentials but still listed.
    pub fn remove_account(&self, email: &str) -> DbResult<()> {
        self.with_transaction(|transaction| {
            // thread_search is an FTS5 virtual table with no FK to threads, so
            // it needs an explicit delete; messages cascade from threads.
            transaction.execute(
                "DELETE FROM thread_search WHERE thread_id IN
                    (SELECT id FROM threads WHERE account_id = ?1)",
                [email],
            )?;
            transaction.execute("DELETE FROM threads WHERE account_id = ?1", [email])?;
            transaction.execute("DELETE FROM mutations WHERE account_id = ?1", [email])?;
            transaction.execute("DELETE FROM tasks WHERE account_id = ?1", [email])?;
            transaction.execute("DELETE FROM sync_state WHERE account_id = ?1", [email])?;
            transaction.execute("DELETE FROM pinned_contacts WHERE account_id = ?1", [email])?;
            transaction.execute(
                "DELETE FROM sync_recovery_threads WHERE account_id = ?1",
                [email],
            )?;
            transaction.execute("DELETE FROM sync_recovery WHERE account_id = ?1", [email])?;
            transaction.execute(
                "DELETE FROM quarantined_messages WHERE account_id = ?1",
                [email],
            )?;
            transaction.execute("DELETE FROM triage_events WHERE account_id = ?1", [email])?;
            transaction.execute("DELETE FROM split_inboxes WHERE account_id = ?1", [email])?;
            transaction.execute("DELETE FROM accounts WHERE email = ?1", [email])?;
            Ok(())
        })
    }

    /// Removes provider-owned/cache state on this installation while keeping
    /// Three Strands-owned workflow records and the synchronized catalog row.
    pub fn disconnect_account_locally(&self, email: &str) -> DbResult<()> {
        self.with_transaction(|transaction| {
            transaction.execute(
                "DELETE FROM thread_search WHERE thread_id IN (SELECT id FROM threads WHERE account_id=?1)",
                [email],
            )?;
            for table in ["threads", "mutations", "sync_state", "pinned_contacts", "sync_recovery_threads", "sync_recovery", "quarantined_messages", "triage_events"] {
                transaction.execute(&format!("DELETE FROM {table} WHERE account_id=?1"), [email])?;
            }
            transaction.execute(
                "UPDATE accounts SET status='needs_reauth',last_synced_at=NULL WHERE email=?1",
                [email],
            )?;
            transaction.execute("INSERT OR IGNORE INTO sync_state(account_id) VALUES(?1)", [email])?;
            Ok(())
        })
    }

    pub fn set_account_color(&self, email: &str, color: &str) -> DbResult<()> {
        let changed = self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE accounts SET color = ?1 WHERE email = ?2",
                params![color, email],
            )?)
        })?;
        if changed == 0 {
            return Err("Account not found".into());
        }
        Ok(())
    }

    pub fn set_account_display_name(
        &self,
        email: &str,
        display_name: Option<&str>,
    ) -> DbResult<()> {
        let normalized = display_name.map(str::trim).filter(|name| !name.is_empty());
        if normalized
            .is_some_and(|name| name.chars().count() > 200 || name.chars().any(char::is_control))
        {
            return Err(
                "Sender name must be 200 characters or fewer and cannot contain control characters"
                    .into(),
            );
        }
        let changed = self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE accounts SET display_name = ?1 WHERE email = ?2",
                params![normalized, email],
            )?)
        })?;
        if changed == 0 {
            return Err("Account not found".into());
        }
        Ok(())
    }

    pub fn reorder_accounts(&self, ordered_emails: &[String]) -> DbResult<()> {
        self.with_transaction(|transaction| {
            for (index, email) in ordered_emails.iter().enumerate() {
                transaction.execute(
                    "UPDATE accounts SET sort_order = ?1 WHERE email = ?2",
                    params![index as i64, email],
                )?;
            }
            Ok(())
        })
    }
}
