//! Calendar account persistence. Calendar OAuth credentials stay in the OS
//! keychain; SQLite stores only the identities needed to discover them again.

use super::{Database, DatabaseError, DbResult};
use crate::models::CalendarAccount;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};

impl Database {
    pub fn list_calendar_accounts(&self) -> DbResult<Vec<CalendarAccount>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT email, connected_at, status, sort_order FROM calendar_accounts ORDER BY sort_order, email")?;
            let rows = statement.query_map([], |row| {
                Ok(CalendarAccount {
                    email: row.get(0)?,
                    connected_at: row.get(1)?,
                    status: row.get(2)?,
                    sort_order: row.get(3)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn adopt_calendar_account(&self, email: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                // A new account goes last; reconnecting one keeps its place.
                "INSERT INTO calendar_accounts(email, connected_at, status, sort_order)
                 VALUES (?1, ?2, 'connected', (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM calendar_accounts))
                 ON CONFLICT(email) DO UPDATE SET connected_at = excluded.connected_at, status='connected'",
                params![email, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn reorder_calendar_accounts(&self, ordered_emails: &[String]) -> DbResult<()> {
        self.with_transaction(|transaction| {
            for (index, email) in ordered_emails.iter().enumerate() {
                transaction.execute(
                    "UPDATE calendar_accounts SET sort_order = ?1 WHERE email = ?2",
                    params![index as i64, email],
                )?;
            }
            Ok(())
        })
    }

    pub fn remove_calendar_account(&self, email: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM calendar_accounts WHERE email = ?1", [email])?;
            Ok(())
        })
    }

    pub fn disconnect_calendar_account_locally(&self, email: &str) -> DbResult<()> {
        let changed = self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE calendar_accounts SET status='needs_reauth' WHERE email=?1",
                [email],
            )?)
        })?;
        if changed == 0 { return Err(DatabaseError::NotFound("Calendar account")); }
        Ok(())
    }

    /// `None` means the account has never chosen calendars, so callers should
    /// initialize the common default (primary only). `Some(vec![])` is an
    /// intentional empty selection and must remain an empty schedule.
    pub fn calendar_selection(&self, email: &str) -> DbResult<Option<Vec<String>>> {
        self.with_connection(|connection| {
            let initialized = connection
                .query_row(
                    "SELECT selection_initialized FROM calendar_accounts WHERE email = ?1",
                    [email],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .ok_or(DatabaseError::NotFound("Calendar account"))?;
            if !initialized {
                return Ok(None);
            }
            let mut statement = connection.prepare(
                "SELECT calendar_id FROM calendar_selections
                 WHERE account_id = ?1 ORDER BY calendar_id",
            )?;
            let rows = statement.query_map([email], |row| row.get(0))?;
            Ok(Some(rows.collect::<Result<Vec<_>, _>>()?))
        })
    }

    pub fn set_calendar_selection(
        &self,
        email: &str,
        calendar_ids: &[String],
    ) -> DbResult<()> {
        self.with_transaction(|transaction| {
            let changed = transaction.execute(
                "UPDATE calendar_accounts SET selection_initialized = 1 WHERE email = ?1",
                [email],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Calendar account"));
            }
            transaction.execute(
                "DELETE FROM calendar_selections WHERE account_id = ?1",
                [email],
            )?;
            for calendar_id in calendar_ids {
                transaction.execute(
                    "INSERT INTO calendar_selections(account_id, calendar_id) VALUES (?1, ?2)",
                    params![email, calendar_id],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_accounts_are_independent_from_mail_accounts() {
        let database = Database::open_memory();
        database.adopt_calendar_account("calendar@example.com").unwrap();

        assert!(database.list_accounts().unwrap().is_empty());
        assert_eq!(
            database.list_calendar_accounts().unwrap()[0].email,
            "calendar@example.com"
        );

        database.remove_calendar_account("calendar@example.com").unwrap();
        assert!(database.list_calendar_accounts().unwrap().is_empty());
    }

    #[test]
    fn calendar_accounts_keep_their_chosen_order_through_a_reconnect() {
        let database = Database::open_memory();
        for email in ["home@example.com", "side@example.com", "work@example.com"] {
            database.adopt_calendar_account(email).unwrap();
        }
        let emails = |database: &Database| -> Vec<String> {
            database.list_calendar_accounts().unwrap().into_iter().map(|account| account.email).collect()
        };
        assert_eq!(emails(&database), ["home@example.com", "side@example.com", "work@example.com"]);

        database
            .reorder_calendar_accounts(&["work@example.com".into(), "home@example.com".into(), "side@example.com".into()])
            .unwrap();
        assert_eq!(emails(&database), ["work@example.com", "home@example.com", "side@example.com"]);

        // Reconnecting refreshes the connection without moving the account.
        database.disconnect_calendar_account_locally("work@example.com").unwrap();
        database.adopt_calendar_account("work@example.com").unwrap();
        assert_eq!(emails(&database), ["work@example.com", "home@example.com", "side@example.com"]);

        database.adopt_calendar_account("new@example.com").unwrap();
        assert_eq!(emails(&database).last().map(String::as_str), Some("new@example.com"));
    }

    #[test]
    fn calendar_selection_distinguishes_default_from_explicitly_empty() {
        let database = Database::open_memory();
        database.adopt_calendar_account("calendar@example.com").unwrap();
        assert_eq!(
            database.calendar_selection("calendar@example.com").unwrap(),
            None
        );

        database
            .set_calendar_selection(
                "calendar@example.com",
                &["primary@example.com".to_string()],
            )
            .unwrap();
        assert_eq!(
            database.calendar_selection("calendar@example.com").unwrap(),
            Some(vec!["primary@example.com".to_string()])
        );

        database
            .set_calendar_selection("calendar@example.com", &[])
            .unwrap();
        assert_eq!(
            database.calendar_selection("calendar@example.com").unwrap(),
            Some(vec![])
        );
    }
}
