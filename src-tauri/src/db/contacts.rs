//! Contact persistence.

use super::*;

impl Database {
    pub fn pin_contact(
        &self,
        account_id: &str,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<(), String> {
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() {
            return Err("Enter an email address".into());
        }
        self.connection()?
            .execute(
                "INSERT INTO pinned_contacts(account_id, email, display_name, pinned_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id, email) DO UPDATE SET display_name = excluded.display_name",
                params![account_id, email, display_name, Utc::now().to_rfc3339()],
            )
            .map_err(display_error)?;
        Ok(())
    }

    pub fn unpin_contact(&self, account_id: &str, email: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "DELETE FROM pinned_contacts WHERE account_id = ?1 AND email = ?2",
                params![account_id, email.trim().to_ascii_lowercase()],
            )
            .map_err(display_error)?;
        Ok(())
    }
}
