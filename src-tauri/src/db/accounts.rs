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
    /// "primary" `GoogleAuth` from `list_accounts().next()`.
    pub fn primary_account_id(&self) -> String {
        self.list_accounts()
            .ok()
            .and_then(|accounts| accounts.into_iter().next())
            .map(|account| account.email)
            .unwrap_or_else(|| crate::auth::LEGACY_KEY.to_string())
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT email, display_name, color, status, provider, sort_order, connected_at, last_synced_at
                 FROM accounts ORDER BY sort_order",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([], account_from_row)
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    /// Ensures an account exists and folds pre-multi-account state onto it.
    pub fn adopt_account(&self, email: &str) -> Result<Account, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE email = ?1)",
                [email],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        if exists {
            transaction
                .execute(
                    "UPDATE accounts SET status = 'connected' WHERE email = ?1",
                    [email],
                )
                .map_err(display_error)?;
        } else {
            let sort_order: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM accounts",
                    [],
                    |row| row.get(0),
                )
                .map_err(display_error)?;
            let color = ACCOUNT_COLORS[(sort_order as usize) % ACCOUNT_COLORS.len()];
            transaction
                .execute(
                    // Every account adopted through this path today comes
                    // from the Gmail OAuth flow.
                    "INSERT INTO accounts(email, color, status, provider, sort_order, connected_at)
                     VALUES (?1, ?2, 'connected', 'gmail', ?3, ?4)",
                    params![email, color, sort_order, Utc::now().to_rfc3339()],
                )
                .map_err(display_error)?;
        }
        for table in ["sync_state", "mutations", "threads", "triage_events"] {
            transaction
                .execute(
                    &format!("UPDATE {table} SET account_id = ?1 WHERE account_id = 'default'"),
                    [email],
                )
                .map_err(display_error)?;
        }
        transaction
            .execute(
                "INSERT OR IGNORE INTO sync_state(account_id) VALUES (?1)",
                [email],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)?;
        drop(connection);
        self.get_account(email)?
            .ok_or_else(|| "Account not found".to_string())
    }

    pub fn get_account(&self, email: &str) -> Result<Option<Account>, String> {
        self.connection()?
            .query_row(
                "SELECT email, display_name, color, status, provider, sort_order, connected_at, last_synced_at
                 FROM accounts WHERE email = ?1",
                [email],
                account_from_row,
            )
            .optional()
            .map_err(display_error)
    }

    /// Atomically records that an account's OAuth grant is unusable and
    /// releases any in-flight mutations back to the durable queue. Claiming
    /// is status-gated, so those mutations remain paused until reconnecting
    /// changes the account back to `connected`.
    pub fn mark_account_needs_reauth(&self, email: &str, error: &str) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute(
                "UPDATE accounts SET status = 'needs_reauth' WHERE email = ?1",
                [email],
            )
            .map_err(display_error)?;
        transaction
            .execute(
                "UPDATE mutations
                 SET state = 'pending', last_error = ?2, next_attempt_at = NULL
                 WHERE account_id = ?1 AND state = 'running'",
                params![email, error],
            )
            .map_err(display_error)?;
        transaction
            .execute(
                "UPDATE sync_state SET last_error = ?2 WHERE account_id = ?1",
                params![email, error],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn account_needs_reauth(&self, email: &str) -> Result<bool, String> {
        self.connection()?
            .query_row(
                "SELECT status = 'needs_reauth' FROM accounts WHERE email = ?1",
                [email],
                |row| row.get(0),
            )
            .optional()
            .map(|status| status.unwrap_or(false))
            .map_err(display_error)
    }

    /// Wipes every trace of an account: credentials are revoked by the
    /// caller only after this commits, so a failure here leaves the account
    /// fully intact rather than stripped of credentials but still listed.
    pub fn remove_account(&self, email: &str) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        // thread_search is an FTS5 virtual table with no FK to threads, so
        // it needs an explicit delete; messages cascade from threads.
        transaction
            .execute(
                "DELETE FROM thread_search WHERE thread_id IN
                    (SELECT id FROM threads WHERE account_id = ?1)",
                [email],
            )
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM threads WHERE account_id = ?1", [email])
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM mutations WHERE account_id = ?1", [email])
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM sync_state WHERE account_id = ?1", [email])
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM pinned_contacts WHERE account_id = ?1", [email])
            .map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM sync_recovery_threads WHERE account_id = ?1",
                [email],
            )
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM sync_recovery WHERE account_id = ?1", [email])
            .map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM quarantined_messages WHERE account_id = ?1",
                [email],
            )
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM triage_events WHERE account_id = ?1", [email])
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM split_inboxes WHERE account_id = ?1", [email])
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM accounts WHERE email = ?1", [email])
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn set_account_color(&self, email: &str, color: &str) -> Result<(), String> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE accounts SET color = ?1 WHERE email = ?2",
                params![color, email],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Account not found".to_string());
        }
        Ok(())
    }

    pub fn set_account_display_name(
        &self,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<(), String> {
        let normalized = display_name.map(str::trim).filter(|name| !name.is_empty());
        if normalized
            .is_some_and(|name| name.chars().count() > 200 || name.chars().any(char::is_control))
        {
            return Err(
                "Sender name must be 200 characters or fewer and cannot contain control characters"
                    .into(),
            );
        }
        let changed = self
            .connection()?
            .execute(
                "UPDATE accounts SET display_name = ?1 WHERE email = ?2",
                params![normalized, email],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Account not found".to_string());
        }
        Ok(())
    }

    pub fn reorder_accounts(&self, ordered_emails: &[String]) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        for (index, email) in ordered_emails.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE accounts SET sort_order = ?1 WHERE email = ?2",
                    params![index as i64, email],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)
    }
}
