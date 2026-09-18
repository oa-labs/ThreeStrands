//! Calendar account persistence. Calendar OAuth credentials stay in the OS
//! keychain; SQLite stores only the identities needed to discover them again.

use super::*;

impl Database {
    pub fn list_calendar_accounts(&self) -> Result<Vec<CalendarAccount>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT email, connected_at FROM calendar_accounts ORDER BY connected_at, email")
            .map_err(display_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(CalendarAccount {
                    email: row.get(0)?,
                    connected_at: row.get(1)?,
                    status: "connected".to_string(),
                })
            })
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    pub fn adopt_calendar_account(&self, email: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT INTO calendar_accounts(email, connected_at) VALUES (?1, ?2)
                 ON CONFLICT(email) DO UPDATE SET connected_at = excluded.connected_at",
                params![email, Utc::now().to_rfc3339()],
            )
            .map_err(display_error)?;
        Ok(())
    }

    pub fn remove_calendar_account(&self, email: &str) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM calendar_accounts WHERE email = ?1", [email])
            .map_err(display_error)?;
        Ok(())
    }

    /// `None` means the account has never chosen calendars, so callers should
    /// initialize the common default (primary only). `Some(vec![])` is an
    /// intentional empty selection and must remain an empty schedule.
    pub fn calendar_selection(&self, email: &str) -> Result<Option<Vec<String>>, String> {
        let connection = self.connection()?;
        let initialized = connection
            .query_row(
                "SELECT selection_initialized FROM calendar_accounts WHERE email = ?1",
                [email],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Calendar account not found".to_string())?;
        if !initialized {
            return Ok(None);
        }
        let mut statement = connection
            .prepare(
                "SELECT calendar_id FROM calendar_selections
                 WHERE account_id = ?1 ORDER BY calendar_id",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([email], |row| row.get(0))
            .map_err(display_error)?;
        Ok(Some(
            rows.collect::<Result<Vec<_>, _>>().map_err(display_error)?,
        ))
    }

    pub fn set_calendar_selection(
        &self,
        email: &str,
        calendar_ids: &[String],
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let changed = transaction
            .execute(
                "UPDATE calendar_accounts SET selection_initialized = 1 WHERE email = ?1",
                [email],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Calendar account not found".to_string());
        }
        transaction
            .execute(
                "DELETE FROM calendar_selections WHERE account_id = ?1",
                [email],
            )
            .map_err(display_error)?;
        for calendar_id in calendar_ids {
            transaction
                .execute(
                    "INSERT INTO calendar_selections(account_id, calendar_id) VALUES (?1, ?2)",
                    params![email, calendar_id],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)
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
