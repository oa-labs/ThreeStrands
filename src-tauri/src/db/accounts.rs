//! Account persistence.

use super::*;

impl Database {
    pub fn list_accounts(&self) -> Result<Vec<Account>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT email, display_name, color, status, sort_order, connected_at, last_synced_at
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
                    "INSERT INTO accounts(email, color, status, sort_order, connected_at)
                     VALUES (?1, ?2, 'connected', ?3, ?4)",
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
                "SELECT email, display_name, color, status, sort_order, connected_at, last_synced_at
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

    pub fn account_is_connected(&self, email: &str) -> Result<bool, String> {
        self.connection()?
            .query_row(
                "SELECT status = 'connected' FROM accounts WHERE email = ?1",
                [email],
                |row| row.get(0),
            )
            .optional()
            // Before the first successful identity migration there is no
            // account row yet; that legacy primary is still allowed to sync.
            .map(|status| status.unwrap_or(true))
            .map_err(display_error)
    }

    /// Durably enters the inactive removal stage before credentials are
    /// touched. Repeating this operation is safe, which lets the user and
    /// startup recovery retry a failed keychain deletion.
    pub fn mark_account_removal_pending(&self, email: &str) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let changed = transaction
            .execute(
                "UPDATE accounts SET status = 'removal_pending' WHERE email = ?1",
                [email],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Account not found".to_string());
        }
        transaction
            .execute(
                "UPDATE outbox_messages
                 SET state = 'failed',
                     error = 'Account removal pending. Restore this draft after reconnecting to send.'
                 WHERE account = ?1 AND state IN ('undo_pending', 'ready')",
                [email],
            )
            .map_err(display_error)?;
        transaction
            .execute(
                "UPDATE mutations
                 SET state = 'pending',
                     last_error = 'Account removal pending',
                     next_attempt_at = NULL
                 WHERE account_id = ?1 AND state = 'running'",
                [email],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn pending_account_removals(&self) -> Result<Vec<String>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT email FROM accounts WHERE status = 'removal_pending' ORDER BY sort_order",
            )
            .map_err(display_error)?;
        let removals = statement
            .query_map([], |row| row.get(0))
            .map_err(display_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display_error)?;
        Ok(removals)
    }

    /// Final stage: credentials have already been deleted, so purge all
    /// remaining local state in one transaction. If this fails, the durable
    /// pending row keeps the account visibly inactive and recovery retries.
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
