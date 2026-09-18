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
}
